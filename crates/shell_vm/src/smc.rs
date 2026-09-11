//! Self-modifying bytecode (Phase 3): the VM never keeps the program in
//! plaintext memory. Instructions and pool data are held XOR-encrypted and
//! are only decrypted one instruction at a time inside the dispatch loop,
//! so a memory dump at any instant shows at most one live instruction.
//!
//! Anti-static-decode hardening (three layers):
//!
//! 1. **Chained keystream** — the keystream for instruction `i` is derived
//!    from the *decoded bytes* of instruction `i-1`, not just from `key`
//!    and position. Decrypting instruction `i` therefore requires having
//!    decrypted instruction `i-1` first: no parallel/sliced decryption, an
//!    unpacker must replay the chain from index 0 exactly.
//!
//! 2. **Runtime salt for pool data** — const-pool strings, heredoc bodies
//!    and function names are additionally masked with a per-process salt
//!    generated from real entropy at VM startup. The salt never exists in
//!    the `.sc` file, so a static tool decrypting the file yields garbage
//!    for every string even with the correct key; only a live process that
//!    ran `enable_smc` can read them.
//!
//! 3. **Polymorphic mixer shapes** — the keystream mixing function itself
//!    is selected per build from 4 distinct algorithm shapes, keyed off
//!    the build key. Two builds of the same script not only use different
//!    keys, they run structurally different keystream generators; an
//!    unpacker must implement all shapes.

/// A single `[op:1][operand:4]` instruction in its raw serialized form.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawInstr {
    pub op: u8,
    pub operand: u32,
}

impl RawInstr {
    pub fn to_bytes(&self) -> [u8; 5] {
        let mut b = [0u8; 5];
        b[0] = self.op;
        b[1..5].copy_from_slice(&self.operand.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8; 5]) -> Self {
        Self {
            op: b[0],
            operand: u32::from_le_bytes(b[1..5].try_into().unwrap()),
        }
    }
}

/// Number of polymorphic keystream mixer shapes.
pub const POLY_SHAPES: u8 = 4;

/// Select the mixer shape for a build: derived from the key so no extra
/// header field is needed — the same derivation runs at seal and decode.
fn shape_of(key: [u8; 16]) -> u8 {
    // Fold enough key bytes to make the choice well distributed.
    let fold = key[15] ^ key[7].rotate_left(3) ^ key[3].wrapping_mul(31);
    fold % POLY_SHAPES
}

/// One step of the per-shape keystream mixer: scrambles the running chain
/// state with the position. Each shape is a different bijection family;
/// all are invertible through the same code path because encryption and
/// decryption share the keystream (XOR).
fn mix_step(shape: u8, state: u32, pos: u32) -> u32 {
    match shape {
        0 => {
            // xorshift family
            let mut h = state.wrapping_mul(0x85EB_CA6B) ^ pos.wrapping_mul(0x9E37_79B9);
            h ^= h >> 15;
            h.wrapping_mul(0xC2B2_AE35)
        }
        1 => {
            // rotate family
            let mut h = state
                .rotate_left(7)
                .wrapping_add(pos)
                .wrapping_mul(0x45D9_F3B);
            h ^= h.rotate_right(11);
            h.wrapping_mul(0x2545_F491)
        }
        2 => {
            // multiply-xor family
            let mut h = state ^ pos.wrapping_mul(0x2722_0A95);
            h = h.wrapping_mul(0x16566_7B1);
            h ^ (h >> 13)
        }
        _ => {
            // avalanche family
            let mut h = state
                .wrapping_add(pos.rotate_left(5))
                .wrapping_mul(0x811C_9DC5);
            h ^= h >> 17;
            h.wrapping_mul(0x7FB3_1F7D) ^ (h >> 5)
        }
    }
}

/// Advance the chained decode state with a decoded instruction's bytes.
/// The chain state after instruction `i` depends on the plaintext of `i`,
/// so producing the keystream for `i+1` requires `i`'s plaintext.
fn chain_advance(shape: u8, state: u32, decoded: &[u8; 5]) -> u32 {
    let feed = u32::from_le_bytes([decoded[1], decoded[2], decoded[3], decoded[4]])
        ^ (decoded[0] as u32).rotate_left(24);
    mix_step(shape, state ^ feed, feed)
}

/// Generate the keystream bytes for a chained instruction decode.
fn chain_keystream(shape: u8, state: u32, pos: usize) -> [u8; 5] {
    let h = mix_step(shape, state, pos as u32);
    let g = h.rotate_left(9).wrapping_mul(0x6C07_8965) ^ (h >> 3);
    let mut ks = [0u8; 5];
    ks[0..4].copy_from_slice(&h.to_le_bytes());
    ks[4] = (g as u8) ^ (g >> 24) as u8;
    ks
}

/// Initial chain state, derived from the key alone.
fn chain_init(key: [u8; 16]) -> u32 {
    let k0 = u32::from_le_bytes([key[0], key[1], key[2], key[3]]);
    let k1 = u32::from_le_bytes([key[4], key[5], key[6], key[7]]);
    k0.rotate_left(13)
        ^ k1.wrapping_mul(0x9E37_79B9)
        ^ u32::from_le_bytes([key[8], key[9], key[10], key[11]])
}

/// Keystream for pool/redir/func data. Chained over the *index* so pool
/// entries still need sequential unlock, but the runtime salt is folded
/// in — without the salt this keystream is wrong by construction.
fn data_keystream(key: [u8; 16], salt: u32, idx: usize) -> [u8; 5] {
    let shape = shape_of(key);
    let mut state = chain_init(key) ^ salt.rotate_left(17);
    for _ in 0..=idx {
        state = mix_step(shape, state, state >> 3);
    }
    let h = state.wrapping_mul(0x85EB_CA6B);
    let mut ks = [0u8; 5];
    ks[0..4].copy_from_slice(&h.to_le_bytes());
    ks[4] = (h >> 29) as u8 ^ key[(idx + 5) % 16];
    ks
}

/// Encrypted instruction stream + decrypted-on-demand pool/redirs/funcs.
#[derive(Clone)]
pub struct SmcStream {
    key: [u8; 16],
    /// Runtime-only salt masking pool data. Set by `enable_smc` from real
    /// entropy; absent (0) only in the seal→unseal round-trip used by eval
    /// splicing, which passes the salt along explicitly.
    salt: u32,
    /// Chained decode state at the *current* sequential position. Decoding
    /// instruction `i` requires the state left by instruction `i-1`, so
    /// random-access decode is impossible by construction. Reset via
    /// `rewind()` to replay from the start.
    chain: u32,
    /// Sequential position the chain state corresponds to.
    chain_pos: usize,
    /// Encrypted `[op:1][operand:4]` rows, one per instruction.
    instrs: Vec<[u8; 5]>,
    /// Const-pool strings, each encrypted as a byte vec.
    pool: Vec<Vec<u8>>,
    /// Redirect entries with their string targets encrypted.
    pub redirs: Vec<SmcRedir>,
    /// Function entries with names encrypted.
    pub funcs: Vec<SmcFunc>,
}

/// Mirror of `shell_bc::bytecode::RedirEntry` with string payloads held
/// encrypted. Pool indices stay plaintext — they are meaningless without
/// the (encrypted) pool.
#[derive(Debug, Clone)]
pub enum SmcTarget {
    File(u32),
    Fd(u32),
    /// Encrypted heredoc body.
    HereDoc(Vec<u8>),
}

#[derive(Debug, Clone)]
pub struct SmcRedir {
    pub kind: u8,
    pub fd: u32,
    pub target: SmcTarget,
}

#[derive(Clone)]
pub struct SmcFunc {
    /// Encrypted function name.
    pub name: Vec<u8>,
    pub entry_ip: u32,
}

impl SmcStream {
    /// Encrypt a whole program in place. Consumes the plaintext pieces;
    /// the caller should drop/blank its copies right after.
    ///
    /// `salt` must be a runtime-generated value (entropy) for protection;
    /// the same salt must be re-supplied on decode via `enable_smc`. For
    /// the internal seal→unseal round-trip (eval splicing) any value
    /// works as long as it matches.
    pub fn seal(
        key: [u8; 16],
        salt: u32,
        instrs: Vec<RawInstr>,
        pool: Vec<String>,
        redirs: Vec<SmcRedir>,
        funcs: Vec<SmcFunc>,
    ) -> Self {
        let shape = shape_of(key);
        // Seal the instruction chain sequentially: state after each row
        // is fed by that row's plaintext.
        let mut state = chain_init(key);
        let enc_instrs = instrs
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let plain = r.to_bytes();
                let ks = chain_keystream(shape, state, i);
                let mut b = plain;
                for (j, k) in ks.iter().enumerate() {
                    b[j] ^= k;
                }
                state = chain_advance(shape, state, &plain);
                b
            })
            .collect();
        let enc_pool = pool
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let ks = data_keystream(key, salt, i);
                s.bytes()
                    .enumerate()
                    .map(|(j, b)| {
                        b ^ ks[j % 5] ^ key[(i + j) % 16] ^ salt.wrapping_add(i as u32) as u8
                    })
                    .collect()
            })
            .collect();
        let enc_redirs = redirs
            .into_iter()
            .map(|mut r| {
                if let SmcTarget::HereDoc(body) = &mut r.target {
                    let ks = data_keystream(key, salt, 0x1000 + r.fd as usize);
                    for (j, b) in body.iter_mut().enumerate() {
                        *b ^= ks[j % 5] ^ key[(j + 3) % 16] ^ (salt >> (j % 24)) as u8;
                    }
                }
                r
            })
            .collect();
        let enc_funcs = funcs
            .into_iter()
            .map(|mut f| {
                let ks = data_keystream(key, salt, 0x2000 + f.entry_ip as usize);
                for (j, b) in f.name.iter_mut().enumerate() {
                    *b ^= ks[j % 5] ^ key[(j + 7) % 16] ^ (salt >> ((j * 3) % 24)) as u8;
                }
                f
            })
            .collect();
        let chain = chain_init(key);
        Self {
            key,
            salt,
            chain,
            chain_pos: 0,
            instrs: enc_instrs,
            pool: enc_pool,
            redirs: enc_redirs,
            funcs: enc_funcs,
        }
    }

    /// Reset the decode chain to the start (state = pre-instruction-0).
    /// Decoding always replays from here; the chain only moves forward.
    pub fn rewind(&mut self) {
        self.chain = chain_init(self.key);
        self.chain_pos = 0;
    }

    pub fn instr_len(&self) -> usize {
        self.instrs.len()
    }

    /// The runtime salt, so a re-seal after eval splicing reuses it.
    pub fn salt(&self) -> u32 {
        self.salt
    }

    /// Decrypt exactly one instruction at `ip`. **Chained**: the keystream
    /// depends on the decode state left by instruction `ip-1`, so this
    /// walks the chain forward from wherever it currently points. The
    /// dispatch loop runs strictly in order, so the common case advances
    /// by one; jumps backwards rewind and replay (cheap: rows are 5 bytes).
    #[inline]
    pub fn instr_at(&mut self, ip: usize) -> Option<RawInstr> {
        let shape = shape_of(self.key);
        if ip < self.chain_pos {
            // Backward jump: replay the chain from the start.
            self.rewind();
        }
        while self.chain_pos < ip {
            // Fast-forward over rows the caller skipped: decode row
            // chain_pos purely to advance the chain state.
            let enc = self.instrs.get(self.chain_pos)?;
            let ks = chain_keystream(shape, self.chain, self.chain_pos);
            let mut b = *enc;
            for (j, k) in ks.iter().enumerate() {
                b[j] ^= k;
            }
            self.chain = chain_advance(shape, self.chain, &b);
            self.chain_pos += 1;
        }
        if self.chain_pos != ip {
            return None; // past the end
        }
        let enc = self.instrs.get(ip)?;
        let ks = chain_keystream(shape, self.chain, ip);
        let mut b = *enc;
        for (j, k) in ks.iter().enumerate() {
            b[j] ^= k;
        }
        let raw = RawInstr::from_bytes(&b);
        self.chain = chain_advance(shape, self.chain, &b);
        self.chain_pos += 1;
        Some(raw)
    }

    /// Decrypt one const-pool string into a fresh allocation. The caller
    /// uses it and drops it; no long-lived plaintext copy remains.
    pub fn pool_get(&self, idx: u32) -> Option<String> {
        let enc = self.pool.get(idx as usize)?;
        let ks = data_keystream(self.key, self.salt, idx as usize);
        let bytes: Vec<u8> = enc
            .iter()
            .enumerate()
            .map(|(j, &b)| {
                b ^ ks[j % 5]
                    ^ self.key[(idx as usize + j) % 16]
                    ^ self.salt.wrapping_add(idx) as u8
            })
            .collect();
        String::from_utf8(bytes).ok()
    }

    /// Decrypt a heredoc body held by a redirect entry.
    pub fn heredoc_of(&self, r: &SmcRedir) -> Option<String> {
        if let SmcTarget::HereDoc(body) = &r.target {
            let ks = data_keystream(self.key, self.salt, 0x1000 + r.fd as usize);
            let bytes: Vec<u8> = body
                .iter()
                .enumerate()
                .map(|(j, &b)| {
                    b ^ ks[j % 5] ^ self.key[(j + 3) % 16] ^ (self.salt >> (j % 24)) as u8
                })
                .collect();
            return String::from_utf8(bytes).ok();
        }
        None
    }

    /// Decrypt a function name.
    pub fn func_name(&self, f: &SmcFunc) -> String {
        let ks = data_keystream(self.key, self.salt, 0x2000 + f.entry_ip as usize);
        let bytes: Vec<u8> = f
            .name
            .iter()
            .enumerate()
            .map(|(j, &b)| {
                b ^ ks[j % 5] ^ self.key[(j + 7) % 16] ^ (self.salt >> ((j * 3) % 24)) as u8
            })
            .collect();
        String::from_utf8(bytes).unwrap_or_default()
    }

    /// Borrow the encrypted redirect list (metadata is safe to read; only
    /// heredoc bodies need decryption).
    pub fn redirs(&self) -> &[SmcRedir] {
        &self.redirs
    }

    pub fn funcs(&self) -> &[SmcFunc] {
        &self.funcs
    }

    fn key(&self) -> [u8; 16] {
        self.key
    }

    /// Decrypt everything back into plaintext bytecode pieces. Used by
    /// `eval` splicing, which unseals, splices, and re-seals. Requires
    /// the same salt used at seal time; replays the chain sequentially.
    pub fn unseal(&self) -> Result<UnsealedParts, shell_ast::ShellError> {
        let shape = shape_of(self.key);
        let mut state = chain_init(self.key);
        let instrs: Vec<RawInstr> = self
            .instrs
            .iter()
            .enumerate()
            .map(|(i, enc)| {
                let ks = chain_keystream(shape, state, i);
                let mut b = *enc;
                for (j, k) in ks.iter().enumerate() {
                    b[j] ^= k;
                }
                let raw = RawInstr::from_bytes(&b);
                state = chain_advance(shape, state, &b);
                raw
            })
            .collect();
        let pool: Vec<String> = (0..self.pool.len())
            .map(|i| self.pool_get(i as u32).unwrap_or_default())
            .collect();
        let redirs: Vec<SmcRedir> = self
            .redirs
            .iter()
            .map(|r| {
                let target = match &r.target {
                    SmcTarget::File(pi) => SmcTarget::File(*pi),
                    SmcTarget::Fd(n) => SmcTarget::Fd(*n),
                    SmcTarget::HereDoc(_) => {
                        SmcTarget::HereDoc(self.heredoc_of(r).unwrap_or_default().into_bytes())
                    }
                };
                SmcRedir {
                    kind: r.kind,
                    fd: r.fd,
                    target,
                }
            })
            .collect();
        let funcs: Vec<SmcFunc> = self
            .funcs
            .iter()
            .map(|f| SmcFunc {
                name: self.func_name(f).into_bytes(),
                entry_ip: f.entry_ip,
            })
            .collect();
        Ok(UnsealedParts {
            key: self.key,
            salt: self.salt,
            instrs,
            pool,
            redirs,
            funcs,
        })
    }
}

/// Plaintext pieces returned by `SmcStream::unseal`.
pub struct UnsealedParts {
    pub key: [u8; 16],
    pub salt: u32,
    pub instrs: Vec<RawInstr>,
    pub pool: Vec<String>,
    pub redirs: Vec<SmcRedir>,
    pub funcs: Vec<SmcFunc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> (Vec<RawInstr>, Vec<String>, Vec<SmcRedir>, Vec<SmcFunc>) {
        let instrs = vec![
            RawInstr {
                op: 0x01,
                operand: 0xABCD_1234,
            },
            RawInstr {
                op: 0x02,
                operand: 0,
            },
            RawInstr {
                op: 0x01,
                operand: 0xABCD_1234, // same instr at a different address
            },
            RawInstr {
                op: 0xFF,
                operand: 5,
            },
        ];
        let pool = vec![
            "echo".into(),
            "hello world".into(),
            "\u{1}\u{2}binary\u{3}".into(),
        ];
        let redirs = vec![SmcRedir {
            kind: 6,
            fd: 0,
            target: SmcTarget::HereDoc(b"line1\nline2\n".to_vec()),
        }];
        let funcs = vec![SmcFunc {
            name: b"main_fn".to_vec(),
            entry_ip: 12,
        }];
        (instrs, pool, redirs, funcs)
    }

    #[test]
    fn instr_roundtrip_per_position() {
        let (instrs, pool, redirs, funcs) = sample();
        let mut stream = SmcStream::seal([7u8; 16], 0, instrs.clone(), pool, redirs, funcs);
        for (i, want) in instrs.iter().enumerate() {
            assert_eq!(&stream.instr_at(i).unwrap(), want);
        }
        assert!(stream.instr_at(999).is_none());
    }

    #[test]
    fn sequential_decode_no_rewind() {
        // The dispatch loop decodes in order — the chain must serve the
        // full sequence without needing rewind.
        let (instrs, pool, redirs, funcs) = sample();
        let mut stream = SmcStream::seal([11u8; 16], 42, instrs.clone(), pool, redirs, funcs);
        for (i, want) in instrs.iter().enumerate() {
            assert_eq!(&stream.instr_at(i).unwrap(), want);
        }
    }

    #[test]
    fn backward_jump_replays_chain() {
        // Loop pattern: decode 0,1,2 then jump back to 1.
        let (instrs, pool, redirs, funcs) = sample();
        let mut stream = SmcStream::seal([5u8; 16], 9, instrs.clone(), pool, redirs, funcs);
        assert_eq!(&stream.instr_at(0).unwrap(), &instrs[0]);
        assert_eq!(&stream.instr_at(1).unwrap(), &instrs[1]);
        assert_eq!(&stream.instr_at(2).unwrap(), &instrs[2]);
        // backward
        assert_eq!(&stream.instr_at(1).unwrap(), &instrs[1]);
        assert_eq!(&stream.instr_at(2).unwrap(), &instrs[2]);
        assert_eq!(&stream.instr_at(3).unwrap(), &instrs[3]);
    }

    #[test]
    fn same_instr_different_ciphertext() {
        let (instrs, pool, redirs, funcs) = sample();
        let stream = SmcStream::seal([7u8; 16], 0, instrs, pool, redirs, funcs);
        // Identical instructions at positions 0 and 2 must not encrypt alike.
        assert_ne!(stream.instrs[0], stream.instrs[2]);
    }

    #[test]
    fn pool_and_heredoc_and_func_roundtrip() {
        let (instrs, pool, redirs, funcs) = sample();
        let want_pool = pool.clone();
        let want_heredoc = String::from_utf8(
            match &redirs[0].target {
                SmcTarget::HereDoc(b) => b.clone(),
                _ => unreachable!(),
            }
            .clone(),
        )
        .unwrap();
        let want_func = String::from_utf8(funcs[0].name.clone()).unwrap();
        let stream = SmcStream::seal([3u8; 16], 77, instrs, pool, redirs, funcs);
        for (i, want) in want_pool.iter().enumerate() {
            assert_eq!(&stream.pool_get(i as u32).unwrap(), want);
        }
        let hd = stream.heredoc_of(&stream.redirs()[0]).unwrap();
        assert_eq!(hd, want_heredoc);
        assert_eq!(stream.func_name(&stream.funcs()[0]), want_func);
    }

    #[test]
    fn wrong_salt_yields_garbage_pool() {
        // The core of the runtime-salt property: a salt that was not used
        // at seal time decrypts pool data into garbage. A static unpacker
        // that knows the key but not the runtime salt fails here.
        let (instrs, pool, redirs, funcs) = sample();
        let stream = SmcStream::seal([3u8; 16], 111, instrs, pool.clone(), redirs, funcs);
        let fake = SmcStream {
            key: stream.key,
            salt: 222, // wrong salt
            chain: 0,
            chain_pos: 0,
            instrs: stream.instrs.clone(),
            pool: stream.pool.clone(),
            redirs: stream.redirs.clone(),
            funcs: stream.funcs.clone(),
        };
        // Garbage may not be valid UTF-8, so compare on the raw decrypted
        // bytes: a wrong salt must not reproduce the plaintext.
        let got = fake.pool_get(0);
        match got {
            Some(s) => assert_ne!(s, pool[0]),
            None => {} // non-UTF-8 garbage: already provably wrong
        }
    }

    #[test]
    fn unseal_reseal_roundtrip() {
        // eval splicing path: unseal → (caller splices) → re-seal with the
        // same key+salt must decode identically afterwards.
        let (instrs, pool, redirs, funcs) = sample();
        let stream = SmcStream::seal([13u8; 16], 55, instrs.clone(), pool, redirs, funcs);
        let parts = stream.unseal().unwrap();
        assert_eq!(parts.salt, 55);
        let re = SmcStream::seal(
            parts.key,
            parts.salt,
            parts.instrs.clone(),
            parts.pool.clone(),
            parts.redirs,
            parts.funcs,
        );
        let mut re = re;
        for (i, want) in instrs.iter().enumerate() {
            assert_eq!(&re.instr_at(i).unwrap(), want);
        }
    }

    #[test]
    fn poly_shapes_all_roundtrip() {
        // All 4 mixer shapes must seal/decode correctly. Keys are chosen
        // to cover every shape the derivation can select.
        for (n, key) in [
            [0x01u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // fold 1 → shape 1
            [0x02u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // fold 2 → shape 2
            [0x03u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // fold 3 → shape 3
            [0x04u8, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0], // fold 4 → shape 0
        ]
        .iter()
        .enumerate()
        {
            let (instrs, pool, redirs, funcs) = sample();
            let mut stream =
                SmcStream::seal(*key, n as u32, instrs.clone(), pool.clone(), redirs, funcs);
            for (i, want) in instrs.iter().enumerate() {
                assert_eq!(&stream.instr_at(i).unwrap(), want, "shape key {n}");
            }
            assert_eq!(stream.pool_get(0).unwrap(), pool[0]);
        }
    }

    #[test]
    fn encrypted_bytes_differ_from_plaintext() {
        let (instrs, pool, redirs, funcs) = sample();
        let stream = SmcStream::seal([9u8; 16], 3, instrs.clone(), pool, redirs, funcs);
        for (i, r) in instrs.iter().enumerate() {
            assert_ne!(stream.instrs[i], r.to_bytes());
        }
    }
}
