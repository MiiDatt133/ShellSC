//! Self-modifying bytecode (Phase 3): the VM never keeps the program in
//! plaintext memory. Instructions and pool data are held XOR-encrypted with
//! a position-dependent keystream and are only decrypted one instruction at
//! a time inside the dispatch loop, so a memory dump at any instant shows
//! at most one live instruction.
//!
//! This is bytecode-level SMC, not native `.text` patching: the CPU never
//! executes these bytes, so no mprotect RWX dance, no I-cache flush, no
//! pipeline stall — the data path stays W^X-clean and portable.

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

/// XOR keystream derived from a per-build key and a position index, so
/// identical instructions at different addresses encrypt differently —
/// no ECB-style repetition for an analyst to exploit.
///
/// Poly VM: the mixing constants and the shift amounts are folded out of
/// the key itself, so two builds with different keys run genuinely
/// different keystream functions — not just different keys through one
/// fixed mixing algorithm.
fn keystream(key: [u8; 16], pos: usize) -> [u8; 5] {
    let k0 = u32::from_le_bytes([key[0], key[1], key[2], key[3]]);
    let k1 = u32::from_le_bytes([key[4], key[5], key[6], key[7]]);
    // Odd multipliers derived from the key: bijections mod 2^32.
    let m0 = k0 | 1;
    let m1 = (k1 >> 11) | 1;
    // Shift amounts in safe ranges, also key-derived.
    let s0 = 9 + (k0 & 7) as u32;
    let s1 = 11 + (k1 & 5) as u32;

    let mut h = (pos as u32).wrapping_mul(m0) ^ k0;
    h ^= h >> s0;
    h = h.wrapping_mul(0x85EB_CA6B);
    h ^= h >> 13;
    let mut g = (pos as u32).wrapping_add(k1) ^ m1;
    g ^= g << s1;
    g = g.wrapping_mul(0xC2B2_AE35);
    g ^= g >> 16;
    let mut ks = [0u8; 5];
    ks[0..4].copy_from_slice(&h.to_le_bytes());
    ks[4] = (g as u8) ^ key[pos % 16];
    ks
}

/// Encrypted instruction stream + decrypted-on-demand pool/redirs/funcs.
#[derive(Clone)]
pub struct SmcStream {
    key: [u8; 16],
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
    pub fn seal(
        key: [u8; 16],
        instrs: Vec<RawInstr>,
        pool: Vec<String>,
        redirs: Vec<SmcRedir>,
        funcs: Vec<SmcFunc>,
    ) -> Self {
        let enc_instrs = instrs
            .iter()
            .enumerate()
            .map(|(i, r)| {
                let mut b = r.to_bytes();
                let ks = keystream(key, i);
                for (j, k) in ks.iter().enumerate() {
                    b[j] ^= k;
                }
                b
            })
            .collect();
        let enc_pool = pool
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let ks = keystream(key, 0x4000_0000 + i);
                s.bytes()
                    .enumerate()
                    .map(|(j, b)| b ^ ks[j % 5] ^ (key[(i + j) % 16] ^ ks[(i + j) % 5]))
                    .collect()
            })
            .collect();
        let enc_redirs = redirs
            .into_iter()
            .map(|mut r| {
                if let SmcTarget::HereDoc(body) = &mut r.target {
                    let ks = keystream(key, 0x8000_0000 + r.fd as usize);
                    for (j, b) in body.iter_mut().enumerate() {
                        *b ^= ks[j % 5] ^ key[(j + 3) % 16];
                    }
                }
                r
            })
            .collect();
        let enc_funcs = funcs
            .into_iter()
            .map(|mut f| {
                let ks = keystream(key, 0xC000_0000 + f.entry_ip as usize);
                for (j, b) in f.name.iter_mut().enumerate() {
                    *b ^= ks[j % 5] ^ key[(j + 7) % 16];
                }
                f
            })
            .collect();
        Self {
            key,
            instrs: enc_instrs,
            pool: enc_pool,
            redirs: enc_redirs,
            funcs: enc_funcs,
        }
    }

    pub fn instr_len(&self) -> usize {
        self.instrs.len()
    }

    /// Decrypt exactly one instruction at `ip`. The returned value lives
    /// on the stack only as long as the caller holds it — the plaintext
    /// never touches the heap.
    #[inline]
    pub fn instr_at(&self, ip: usize) -> Option<RawInstr> {
        let enc = self.instrs.get(ip)?;
        let mut b = *enc;
        let ks = keystream(self.key, ip);
        for (j, k) in ks.iter().enumerate() {
            b[j] ^= k;
        }
        Some(RawInstr::from_bytes(&b))
    }

    /// Decrypt one const-pool string into a fresh allocation. The caller
    /// uses it and drops it; no long-lived plaintext copy remains.
    pub fn pool_get(&self, idx: u32) -> Option<String> {
        let enc = self.pool.get(idx as usize)?;
        let ks = keystream(self.key, 0x4000_0000 + idx as usize);
        let bytes: Vec<u8> = enc
            .iter()
            .enumerate()
            .map(|(j, &b)| {
                b ^ ks[j % 5] ^ (self.key[(idx as usize + j) % 16] ^ ks[(idx as usize + j) % 5])
            })
            .collect();
        String::from_utf8(bytes).ok()
    }

    /// Decrypt a heredoc body held by a redirect entry.
    pub fn heredoc_of(&self, r: &SmcRedir) -> Option<String> {
        if let SmcTarget::HereDoc(body) = &r.target {
            let ks = keystream(self.key, 0x8000_0000 + r.fd as usize);
            let bytes: Vec<u8> = body
                .iter()
                .enumerate()
                .map(|(j, &b)| b ^ ks[j % 5] ^ self.key[(j + 3) % 16])
                .collect();
            return String::from_utf8(bytes).ok();
        }
        None
    }

    /// Decrypt a function name.
    pub fn func_name(&self, f: &SmcFunc) -> String {
        let ks = keystream(self.key, 0xC000_0000 + f.entry_ip as usize);
        let bytes: Vec<u8> = f
            .name
            .iter()
            .enumerate()
            .map(|(j, &b)| b ^ ks[j % 5] ^ self.key[(j + 7) % 16])
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

    /// The sealing key — needed to re-seal after a plaintext splice (eval).
    pub fn key(&self) -> [u8; 16] {
        self.key
    }

    /// Decrypt everything back into plaintext bytecode pieces. Used by
    /// `eval` splicing, which unseals, splices, and re-seals.
    pub fn unseal(&self) -> Result<UnsealedParts, shell_ast::ShellError> {
        let instrs: Vec<RawInstr> = self
            .instrs
            .iter()
            .enumerate()
            .map(|(i, enc)| {
                let mut b = *enc;
                let ks = keystream(self.key, i);
                for (j, k) in ks.iter().enumerate() {
                    b[j] ^= k;
                }
                RawInstr::from_bytes(&b)
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
        let stream = SmcStream::seal([7u8; 16], instrs.clone(), pool, redirs, funcs);
        for (i, want) in instrs.iter().enumerate() {
            assert_eq!(&stream.instr_at(i).unwrap(), want);
        }
        assert!(stream.instr_at(999).is_none());
    }

    #[test]
    fn same_instr_different_ciphertext() {
        let (instrs, pool, redirs, funcs) = sample();
        let stream = SmcStream::seal([7u8; 16], instrs, pool, redirs, funcs);
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
        let stream = SmcStream::seal([3u8; 16], instrs, pool, redirs, funcs);
        for (i, want) in want_pool.iter().enumerate() {
            assert_eq!(&stream.pool_get(i as u32).unwrap(), want);
        }
        let hd = stream.heredoc_of(&stream.redirs()[0]).unwrap();
        assert_eq!(hd, want_heredoc);
        assert_eq!(stream.func_name(&stream.funcs()[0]), want_func);
    }

    #[test]
    fn encrypted_bytes_differ_from_plaintext() {
        let (instrs, pool, redirs, funcs) = sample();
        let stream = SmcStream::seal([9u8; 16], instrs.clone(), pool, redirs, funcs);
        for (i, r) in instrs.iter().enumerate() {
            assert_ne!(stream.instrs[i], r.to_bytes());
        }
    }
}
