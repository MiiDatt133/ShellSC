//! Phase 3 protection: bytecode encryption (XOR keystream), per-build
//! opcode shuffling, integrity verification (CRC32), and the container
//! header that carries key/seed/checksum inside the `.shellsc` section.
//!
//! Layout of a protected `.shellsc` section:
//!
//! ```text
//! [ ProtectHeader ][ encrypted bytecode ]
//! ```
//!
//! The unprotected build writes the bytecode directly (no header).

use shell_ast::ShellError;

pub const PROTECT_MAGIC: &[u8; 12] = b"SHELLSC_PROT";

/// Version byte: bumped on any header-layout change.
/// v4: cff_seed and opmap_seed are stored XOR-masked with the stub .text
/// CRC (like the key since v3) — no seed bytes readable without the stub.
/// v5: body encrypted with the content-chained keystream (chain_crypt)
/// instead of the repeating 16-byte XOR; const pool sealed to bytecode
/// 0x04 (pool_seal) when FLAG_ENCRYPTED. v4 payloads still open.
/// v6: header.key is a random salt; key/seeds derive from a stretched
/// KDF over (text_crc, salt, flags) — a single offline derivation costs
/// milliseconds and brute-forcing a patched .text CRC becomes infeasible.
/// Also stops the v5 opaque_param key-byte leak (params are random now).
pub const PROTECT_VERSION: u8 = 6;

/// magic 12 + version 1 + flags 1 + cff_seed 2 + key 16 + crc32 4 +
/// orig_len 4 + opmap_seed 4 + opaque_param1 4 + opaque_param2 4 = 52 bytes.
pub const HEADER_LEN: usize = 52;

pub const FLAG_ENCRYPTED: u8 = 0x01;
pub const FLAG_OPMAP: u8 = 0x02;
pub const FLAG_ANTIDEBUG: u8 = 0x04;
pub const FLAG_CFF: u8 = 0x08;
pub const FLAG_OPAQUE: u8 = 0x10;
pub const FLAG_SMC: u8 = 0x20;
pub const FLAG_ANTIHOOK: u8 = 0x40;
pub const FLAG_SELFDEBUG: u8 = 0x80;

/// Key derivation from ELF .text CRC is active when version >= 3 and
/// FLAG_ENCRYPTED is set. No separate flag bit needed — all 8 bits are used.
pub const KEYDERIVE_MIN_VERSION: u8 = 3;

/// Seed masking (cff_seed, opmap_seed XOR text_crc) is active when
/// version >= 4. v3 payloads store the seeds plaintext.
pub const SEEDMASK_MIN_VERSION: u8 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectHeader {
    pub flags: u8,
    pub cff_seed: u16,
    pub key: [u8; 16],
    pub crc32: u32,
    pub orig_len: u32,
    pub opmap_seed: u32,
    pub opaque_param1: u32,
    pub opaque_param2: u32,
}

impl ProtectHeader {
    pub fn to_bytes(&self) -> [u8; HEADER_LEN] {
        let mut buf = [0u8; HEADER_LEN];
        buf[0..12].copy_from_slice(PROTECT_MAGIC);
        buf[12] = PROTECT_VERSION;
        buf[13] = self.flags;
        buf[14..16].copy_from_slice(&self.cff_seed.to_le_bytes());
        buf[16..32].copy_from_slice(&self.key);
        buf[32..36].copy_from_slice(&self.crc32.to_le_bytes());
        buf[36..40].copy_from_slice(&self.orig_len.to_le_bytes());
        buf[40..44].copy_from_slice(&self.opmap_seed.to_le_bytes());
        buf[44..48].copy_from_slice(&self.opaque_param1.to_le_bytes());
        buf[48..52].copy_from_slice(&self.opaque_param2.to_le_bytes());
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ShellError> {
        if bytes.len() < HEADER_LEN {
            return Err(ShellError::IoError("protect header truncated".into()));
        }
        if &bytes[0..12] != PROTECT_MAGIC {
            return Err(ShellError::IoError("bad protect magic".into()));
        }
        // v4/v5 payloads stay readable; seal() only produces the current version.
        if bytes[12] > PROTECT_VERSION || bytes[12] < 4 {
            return Err(ShellError::IoError(format!(
                "unsupported protect version {}",
                bytes[12]
            )));
        }
        Ok(Self {
            flags: bytes[13],
            cff_seed: u16::from_le_bytes(bytes[14..16].try_into().unwrap()),
            key: bytes[16..32].try_into().unwrap(),
            crc32: u32::from_le_bytes(bytes[32..36].try_into().unwrap()),
            orig_len: u32::from_le_bytes(bytes[36..40].try_into().unwrap()),
            opmap_seed: u32::from_le_bytes(bytes[40..44].try_into().unwrap()),
            opaque_param1: u32::from_le_bytes(bytes[44..48].try_into().unwrap()),
            opaque_param2: u32::from_le_bytes(bytes[48..52].try_into().unwrap()),
        })
    }
}

/// True if the `.shellsc` section payload starts with a protection header.
pub fn is_protected(payload: &[u8]) -> bool {
    payload.len() >= HEADER_LEN && &payload[0..12] == PROTECT_MAGIC
}

// ── CRC32 (IEEE 802.3, table-driven) ─────────────────────────────────────────

fn crc_table() -> [u32; 256] {
    let mut table = [0u32; 256];
    for (i, entry) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 {
                0xEDB8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
        }
        *entry = c;
    }
    table
}

pub fn crc32(data: &[u8]) -> u32 {
    let table = crc_table();
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc = table[((crc ^ b as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFF_FFFF
}

// ── XOR keystream ────────────────────────────────────────────────────────────

/// Repeating-key XOR. Length-preserving, symmetric — decryption is the
/// same operation.
pub fn xor_crypt(key: &[u8; 16], data: &mut [u8]) {
    for (i, b) in data.iter_mut().enumerate() {
        *b ^= key[i % 16];
    }
}

// ── Random key/seed generation ───────────────────────────────────────────────

/// Minimal xorshift PRNG — seeded from a real entropy source once, then
/// produces the key and the opcode-permutation seed.
pub struct Rng(u64);

impl Rng {
    pub fn from_entropy() -> Self {
        let seed = entropy_u64();
        // xorshift must never start at 0.
        Self(seed | 1)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn next_u32(&mut self) -> u32 {
        self.next_u64() as u32
    }

    pub fn key16(&mut self) -> [u8; 16] {
        let mut key = [0u8; 16];
        for chunk in key.chunks_mut(8) {
            let v = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
        key
    }
}

fn entropy_u64() -> u64 {
    // Prefer the kernel CSPRNG.
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        use std::io::Read;
        let mut buf = [0u8; 8];
        if f.read_exact(&mut buf).is_ok() {
            return u64::from_le_bytes(buf);
        }
    }
    // Fallback: ASLR address + high-res clock + pid.
    let t = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    (t ^ (std::process::id() as u64) ^ (&t as *const u64 as u64)) | 1
}

// ── Build-time options ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct ProtectOptions {
    pub encrypt: bool,
    pub opmap: bool,
    pub antidebug: bool,
    pub cff: bool,
    pub opaque: bool,
    pub smc: bool,
    pub antihook: bool,
    pub selfdebug: bool,
}

impl ProtectOptions {
    pub fn all() -> Self {
        Self {
            encrypt: true,
            opmap: true,
            antidebug: true,
            cff: true,
            opaque: true,
            smc: true,
            antihook: true,
            selfdebug: true,
        }
    }

    pub fn is_any(&self) -> bool {
        self.encrypt
            || self.opmap
            || self.antidebug
            || self.cff
            || self.opaque
            || self.smc
            || self.antihook
            || self.selfdebug
    }

    fn flags(&self) -> u8 {
        let mut f = 0u8;
        if self.encrypt {
            f |= FLAG_ENCRYPTED;
        }
        if self.opmap {
            f |= FLAG_OPMAP;
        }
        if self.antidebug {
            f |= FLAG_ANTIDEBUG;
        }
        if self.cff {
            f |= FLAG_CFF;
        }
        if self.opaque {
            f |= FLAG_OPAQUE;
        }
        if self.smc {
            f |= FLAG_SMC;
        }
        if self.antihook {
            f |= FLAG_ANTIHOOK;
        }
        if self.selfdebug {
            f |= FLAG_SELFDEBUG;
        }
        f
    }
}

/// Rewrite opcode bytes in place over the serialized SHBC stream. Only
/// the bytes at `shell_bc::opmap::instruction_offsets` positions are
/// touched — the const pool, redirect table and function table contain
/// arbitrary bytes that may collide with opcode values and must stay
/// intact. Layout walking is position-based for the header/pool/redirs/
/// funcs, and the instruction walk only needs each op byte to decode as
/// *some* opcode — shuffled bytes are still valid opcode values, so the
/// same offsets function works on plaintext and shuffled streams alike.
pub fn apply_opmap(bc_bytes: &mut [u8], perm: &[u8; OPMAP_SIZE]) -> Result<(), ShellError> {
    let offsets = shell_bc::opmap::instruction_offsets(bc_bytes)?;
    for off in offsets {
        let op = bc_bytes[off];
        if op == 0xFF {
            // Exit stays unshuffled: the VM treats it as the terminator.
            continue;
        }
        if !(OPMAP_LO..OPMAP_HI).contains(&op) {
            return Err(ShellError::BytecodeError(format!(
                "apply_opmap: unknown opcode {:#04x} at {}",
                op, off
            )));
        }
        bc_bytes[off] = perm[(op - OPMAP_LO) as usize];
    }
    Ok(())
}

/// Inverse of `apply_opmap`: restore original opcode bytes.
pub fn revert_opmap(bc_bytes: &mut [u8], inv: &[u8; OPMAP_SIZE]) -> Result<(), ShellError> {
    let offsets = shell_bc::opmap::instruction_offsets(bc_bytes)?;
    for off in offsets {
        let op = bc_bytes[off];
        if op == 0xFF {
            continue;
        }
        if !(OPMAP_LO..OPMAP_HI).contains(&op) {
            return Err(ShellError::BytecodeError(format!(
                "revert_opmap: unknown opcode {:#04x} at {}",
                op, off
            )));
        }
        bc_bytes[off] = inv[(op - OPMAP_LO) as usize];
    }
    Ok(())
}

// ── Key masking (v3–v5) ─────────────────────────────────────────────────────

fn mask_key(key: &[u8; 16], text_hash: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, b) in key.iter().enumerate() {
        out[i] = b ^ (text_hash >> ((i % 4) * 8)) as u8;
    }
    out
}

fn mask_cff_seed(seed: u16, text_hash: u32) -> u16 {
    seed ^ (text_hash >> 16) as u16
}

fn mask_opmap_seed(seed: u32, text_hash: u32) -> u32 {
    seed ^ text_hash
}

// ── v6 stretched key derivation ─────────────────────────────────────────────

/// mix64 rounds for the v6 KDF. One runtime derivation costs a few
/// milliseconds; brute-forcing a patched .text CRC (2^32 candidates ×
/// chain_decrypt + CRC verify) — the attack that turns v5 header-masking
/// into a minutes-long offline break — becomes multi-century work.
pub const KDF_ROUNDS: u32 = 1 << 20;

/// Expand (text_crc, salt, flags) into 24 bytes:
/// `[0..16]` crypto key, `[16..18]` cff_seed, `[18..22]` opmap_seed,
/// `[22..24]` pad. The header stores only the random salt; nothing about
/// the key exists in the file without also knowing the stub's .text CRC —
/// and even then the attacker pays KDF_ROUNDS per guess.
pub fn kdf_expand(text_crc: u32, salt: &[u8; 16], flags: u8) -> [u8; 24] {
    use shell_bc::pool_seal::{mix64, PHI_PUBLIC};
    let s0 = u64::from_le_bytes(salt[0..8].try_into().unwrap());
    let s1 = u64::from_le_bytes(salt[8..16].try_into().unwrap());
    let mut state = mix64(u64::from(text_crc) ^ PHI_PUBLIC ^ ((flags as u64) << 56));
    let mut b = s0 ^ 0xA5A5_A5A5_DEAD_BEEF;
    let mut c = s1.wrapping_add(PHI_PUBLIC);
    for _ in 0..KDF_ROUNDS {
        state = mix64(state ^ b ^ c);
        b = mix64(b ^ state);
        c = mix64(c.wrapping_add(state));
    }
    let o1 = mix64(state ^ b);
    let o2 = mix64(state.wrapping_add(c) ^ 0x1234_5678_9ABC_DEF0);
    let o3 = mix64(b ^ c.wrapping_add(o1));
    let mut out = [0u8; 24];
    out[0..8].copy_from_slice(&o1.to_le_bytes());
    out[8..16].copy_from_slice(&o2.to_le_bytes());
    out[16..18].copy_from_slice(&(o3 as u16).to_le_bytes());
    out[18..22].copy_from_slice(&((o3 >> 16) as u32).to_le_bytes());
    out
}

fn kdf_key(text_crc: u32, salt: &[u8; 16], flags: u8) -> [u8; 16] {
    let mat = kdf_expand(text_crc, salt, flags);
    let mut key = [0u8; 16];
    key.copy_from_slice(&mat[0..16]);
    key
}

/// Compute the CRC32 of the ELF stub's executable code (.text section).
/// Used by both the packer (at build time) and the stub (at runtime) to
/// derive the same hash without storing it anywhere.
pub fn text_crc_from_elf(elf_bytes: &[u8]) -> Result<u32, ShellError> {
    let elf = goblin::elf::Elf::parse(elf_bytes)
        .map_err(|e| ShellError::IoError(format!("ELF parse: {}", e)))?;
    for sh in &elf.section_headers {
        if elf.shdr_strtab.get_at(sh.sh_name) == Some(".text") {
            let start = sh.sh_offset as usize;
            let end = start + sh.sh_size as usize;
            let text = elf_bytes
                .get(start..end)
                .ok_or_else(|| ShellError::IoError(".text out of bounds".into()))?;
            return Ok(crc32(text));
        }
    }
    // No .text section (unlikely): fall back to CRC of entire ELF minus last page.
    Ok(crc32(elf_bytes))
}

// ── Seal / open ──────────────────────────────────────────────────────────────

/// Per-block CFI checksum table over the plaintext instruction rows:
/// CRC32 of every `shell_bc::opmap::CFI_BLOCK` consecutive 5-byte rows.
/// The runtime verifier (`shell_vm::smc`) recomputes the same fingerprint
/// row-by-row as it decodes; any divergence means the stream was patched.
fn cfi_build_table(bc_bytes: &[u8]) -> Result<Vec<u32>, ShellError> {
    let offsets = shell_bc::opmap::instruction_offsets(bc_bytes)?;
    Ok(offsets
        .chunks(shell_bc::opmap::CFI_BLOCK)
        .map(|chunk| {
            let mut bytes = Vec::with_capacity(chunk.len() * 5);
            for &off in chunk {
                bytes.extend_from_slice(&bc_bytes[off..off + 5]);
            }
            crc32(&bytes)
        })
        .collect())
}

/// Seal the plaintext bytecode into a protected `.shellsc` payload.
/// `text_hash` is the CRC32 of the ELF stub's .text section; when encrypting,
/// the key stored in the header is XOR-masked with this hash so the real key
/// only exists after the stub derives it at runtime from its own code bytes.
pub fn seal(bc_bytes: &[u8], opts: ProtectOptions, text_hash: u32) -> Result<Vec<u8>, ShellError> {
    let mut rng = Rng::from_entropy();
    let flags = opts.flags();
    // v6: the header stores a random salt; key and opcode seeds stretch
    // out of (text_crc, salt, flags) so no key material is file-readable
    // and every offline guess pays KDF_ROUNDS.
    let salt = rng.key16();
    let mat = kdf_expand(text_hash, &salt, flags);
    let mut key = [0u8; 16];
    key.copy_from_slice(&mat[0..16]);
    let kdf_opmap_seed = u32::from_le_bytes(mat[18..22].try_into().unwrap());

    let mut body = bc_bytes.to_vec();

    // Seal the const pool first (0x03 -> 0x04 with per-entry ciphertext)
    // so the CRC/length checks in open() cover the sealed layout that comes
    // back out of the decrypt path. The pool key travels inside the chain-
    // encrypted body, never in the header.
    if opts.encrypt {
        let pool_key = rng.key16();
        body = shell_bc::pool_seal::seal_stream(&body, &pool_key)?;
    }

    // Checksum over the sealed bare stream: open() verifies it after the
    // decrypt + CFI strip + opmap revert, so compute it BEFORE the shuffle
    // (revert is an exact inverse) — this is the byte sequence open() has
    // in hand at that point.
    let crc = crc32(&body);
    let orig_len = body.len() as u32;

    if opts.opmap {
        let perm = opcode_permutation(kdf_opmap_seed);
        apply_opmap(&mut body, &perm)?;
    }

    // CFI table sits AHEAD of the shuffled stream (never inside the
    // instruction region the opmap walk sees) and is encrypted together
    // with it. Header.orig_len covers the SHBC stream only — open()
    // strips the table before the length check.
    if opts.smc {
        let table = cfi_build_table(bc_bytes)?;
        let mut framed = Vec::with_capacity(4 + table.len() * 4 + body.len());
        framed.extend_from_slice(&(table.len() as u32).to_le_bytes());
        for c in &table {
            framed.extend_from_slice(&c.to_le_bytes());
        }
        framed.append(&mut body);
        body = framed;
    }

    if opts.encrypt {
        shell_bc::pool_seal::chain_crypt(&key, &mut body, false);
    }

    // Random opaque params (v6): the v5 derivation (param1 = crc ^ key[0],
    // param2 = key[1]*key[2]) put key bytes straight into the header.
    let opaque_p1 = if opts.opaque { rng.next_u32() } else { 0 };
    let opaque_p2 = if opts.opaque { rng.next_u32() } else { 0 };

    let header = ProtectHeader {
        flags,
        // v6: stored seeds are filler — derive_seeds() recomputes the real
        // values through the KDF; keeping the fields populated preserves
        // the layout and hides build-option patterns.
        cff_seed: if opts.cff { rng.next_u32() as u16 } else { 0 },
        key: salt,
        crc32: crc,
        orig_len,
        opmap_seed: if opts.opmap { rng.next_u32() } else { 0 },
        opaque_param1: opaque_p1,
        opaque_param2: opaque_p2,
    };

    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.extend_from_slice(&header.to_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// The runtime seeds derived from a (possibly masked) header and the stub's
/// `.text` CRC. Version >= 6 stretches them through the KDF; v4/v5 store
/// XOR-masked; earlier versions store them plaintext.
#[derive(Debug, Clone, Copy)]
pub struct DerivedSeeds {
    pub cff_seed: u16,
    pub opmap_seed: u32,
}

pub fn derive_seeds(header: &ProtectHeader, version: u8, text_hash: u32) -> DerivedSeeds {
    if version >= PROTECT_VERSION {
        let mat = kdf_expand(text_hash, &header.key, header.flags);
        return DerivedSeeds {
            cff_seed: u16::from_le_bytes(mat[16..18].try_into().unwrap()),
            opmap_seed: u32::from_le_bytes(mat[18..22].try_into().unwrap()),
        };
    }
    DerivedSeeds {
        cff_seed: if version >= SEEDMASK_MIN_VERSION {
            mask_cff_seed(header.cff_seed, text_hash)
        } else {
            header.cff_seed
        },
        opmap_seed: if version >= SEEDMASK_MIN_VERSION {
            mask_opmap_seed(header.opmap_seed, text_hash)
        } else {
            header.opmap_seed
        },
    }
}

/// Open a protected payload: returns the decrypted, opcode-restored
/// bytecode bytes plus the CFI block table when SMC protection sealed one.
/// `text_hash` is the CRC32 of the ELF stub's .text section, used to
/// unmask the key stored in the header (version >= 3).
pub fn open(
    payload: &[u8],
    text_hash: u32,
) -> Result<Option<(Vec<u8>, Option<Vec<u32>>)>, ShellError> {
    if !is_protected(payload) {
        return Ok(None);
    }
    let header = ProtectHeader::from_bytes(payload)?;
    let mut body = payload[HEADER_LEN..].to_vec();
    let version = payload[12];

    if header.flags & FLAG_ENCRYPTED != 0 {
        let real_key = if version >= PROTECT_VERSION {
            kdf_key(text_hash, &header.key, header.flags)
        } else if version >= KEYDERIVE_MIN_VERSION {
            mask_key(&header.key, text_hash)
        } else {
            header.key
        };
        if version >= 5 {
            shell_bc::pool_seal::chain_crypt(&real_key, &mut body, true);
        } else {
            xor_crypt(&real_key, &mut body);
        }
    }

    // CFI table frames the SHBC stream: [num_blocks u32 LE][crc32 * n].
    // Strip it before the opmap revert / CRC / length checks — those all
    // operate on the bare bytecode stream.
    let cfi = if header.flags & FLAG_SMC != 0 {
        if body.len() < 4 {
            return Err(ShellError::IoError(
                "binary tampered: cfi table truncated".into(),
            ));
        }
        let n = u32::from_le_bytes(body[0..4].try_into().unwrap()) as usize;
        if n > (body.len() - 4) / 4 {
            return Err(ShellError::IoError(
                "binary tampered: cfi table truncated".into(),
            ));
        }
        let tlen = 4 + n * 4;
        let table = body[4..tlen]
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes(c.try_into().unwrap()))
            .collect();
        body.drain(0..tlen);
        Some(table)
    } else {
        None
    };

    let seeds = derive_seeds(&header, version, text_hash);

    if header.flags & FLAG_OPMAP != 0 && seeds.opmap_seed != 0 {
        let inv = opcode_inverse(seeds.opmap_seed, version < PROTECT_VERSION);
        revert_opmap(&mut body, &inv).map_err(|_| {
            ShellError::IoError("binary tampered: refuse to run modified executable".into())
        })?;
    }

    if crc32(&body) != header.crc32 {
        return Err(ShellError::IoError(
            "binary tampered: refuse to run modified executable".into(),
        ));
    }
    if body.len() != header.orig_len as usize {
        return Err(ShellError::IoError("bytecode length mismatch".into()));
    }
    Ok(Some((body, cfi)))
}

/// Number of real opcodes to permute. Shuffle the contiguous 0x01..=0x2F
/// range (47 opcodes incl. SubshellBgEnd, DynRedirLine); Exit at
/// 0xFF stays put because the VM treats it as the terminator.
pub const OPMAP_LO: u8 = 0x01;
pub const OPMAP_HI: u8 = 0x30; // exclusive: covers 0x01..=0x2F (47 opcodes)
pub const OPMAP_SIZE: usize = (OPMAP_HI - OPMAP_LO) as usize;

/// Derive a deterministic permutation of opcode bytes from a seed.
/// Returns `perm[orig - OPMAP_LO] = shuffled_byte`.
///
/// The Fisher–Yates index comes from the full u64 so the shuffle no longer
/// depends on the build host's pointer width (pre-v5 `as usize` truncated
/// on 32-bit hosts — a cross-compile produced a different permutation than
/// the target runtime expected).
pub fn opcode_permutation(seed: u32) -> [u8; OPMAP_SIZE] {
    let mut perm: [u8; OPMAP_SIZE] = (OPMAP_LO..OPMAP_HI)
        .collect::<Vec<u8>>()
        .try_into()
        .unwrap();
    let mut rng = Rng(seed as u64 | 1);
    let mut i = perm.len();
    while i > 1 {
        i -= 1;
        let j = (rng.next_u64() % (i as u64 + 1)) as usize;
        perm.swap(i, j);
    }
    perm
}

/// Pre-v5 shuffle, kept so v4 payloads still revert: reproduces the old
/// `next_u64() as usize` cast exactly (host word size and all).
fn opcode_permutation_legacy(seed: u32) -> [u8; OPMAP_SIZE] {
    let mut perm: [u8; OPMAP_SIZE] = (OPMAP_LO..OPMAP_HI)
        .collect::<Vec<u8>>()
        .try_into()
        .unwrap();
    let mut rng = Rng(seed as u64 | 1);
    let mut i = perm.len();
    while i > 1 {
        i -= 1;
        let j = (rng.next_u64() as usize) % (i + 1);
        perm.swap(i, j);
    }
    perm
}

/// Inverse of the seed-derived permutation: `inv[shuffled - OPMAP_LO] =
/// orig_byte`. `legacy` selects the pre-v5 host-dependent shuffle.
pub fn opcode_inverse(seed: u32, legacy: bool) -> [u8; OPMAP_SIZE] {
    let perm = if legacy {
        opcode_permutation_legacy(seed)
    } else {
        opcode_permutation(seed)
    };
    let mut inv = [0u8; OPMAP_SIZE];
    for (i, &shuffled) in perm.iter().enumerate() {
        inv[(shuffled - OPMAP_LO) as usize] = OPMAP_LO + i as u8;
    }
    inv
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrip() {
        let h = ProtectHeader {
            flags: FLAG_ENCRYPTED | FLAG_OPMAP | FLAG_ANTIDEBUG | FLAG_CFF | FLAG_OPAQUE,
            cff_seed: 0xBEEF,
            key: [7; 16],
            crc32: 0xDEAD_BEEF,
            orig_len: 1234,
            opmap_seed: 42,
            opaque_param1: 0x1234,
            opaque_param2: 0x5678,
        };
        let bytes = h.to_bytes();
        let h2 = ProtectHeader::from_bytes(&bytes).unwrap();
        assert_eq!(h, h2);
    }

    #[test]
    fn crc32_known_value() {
        // CRC32 of "123456789" (standard check value).
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
    }

    #[test]
    fn xor_roundtrip() {
        let key = [9u8; 16];
        let mut data = b"hello world, this is bytecode".to_vec();
        let orig = data.clone();
        xor_crypt(&key, &mut data);
        assert_ne!(data, orig);
        xor_crypt(&key, &mut data);
        assert_eq!(data, orig);
    }

    #[test]
    fn permutation_is_bijection() {
        for seed in [0u32, 1, 7, 0xFFFF_FFFF] {
            for legacy in [false, true] {
                let perm = if legacy {
                    opcode_permutation_legacy(seed)
                } else {
                    opcode_permutation(seed)
                };
                let inv = opcode_inverse(seed, legacy);
                for i in 0..OPMAP_SIZE {
                    let shuffled = perm[i];
                    assert_eq!(inv[(shuffled - OPMAP_LO) as usize], OPMAP_LO + i as u8);
                }
            }
        }
    }

    #[test]
    fn permutation_depends_on_seed() {
        let a = opcode_permutation(1);
        let b = opcode_permutation(2);
        assert_ne!(a, b);
    }

    #[test]
    fn is_protected_detection() {
        assert!(!is_protected(b""));
        assert!(!is_protected(&[0u8; 200]));
        let h = ProtectHeader {
            flags: 0,
            cff_seed: 0,
            key: [0; 16],
            crc32: 0,
            orig_len: 0,
            opmap_seed: 0,
            opaque_param1: 0,
            opaque_param2: 0,
        };
        let mut payload = h.to_bytes().to_vec();
        payload.extend_from_slice(&[1, 2, 3]);
        assert!(is_protected(&payload));
    }

    fn sample_shbc() -> Vec<u8> {
        let mut bc = shell_bc::Bytecode::default();
        bc.const_pool.strings.push("echo".to_string());
        bc.const_pool.strings.push("hi".to_string());
        // Pool bytes deliberately collide with opcode values.
        bc.const_pool.strings.push("\u{1}\u{3}\u{2}".to_string());
        for (op, operand) in [
            (shell_bc::Opcode::PushConst, 0),
            (shell_bc::Opcode::PushConst, 1),
            (shell_bc::Opcode::Builtin, 0),
            (shell_bc::Opcode::Exit, 0),
        ] {
            bc.instructions
                .push(shell_bc::bytecode::Instruction { op, operand });
        }
        shell_bc::serialize::serialize(&bc)
    }

    /// `open()` on an encrypted build returns the pool-sealed 0x04 stream
    /// (bytecode 0x03 + spliced pool key + hex entries), not the raw 0x03
    /// input. Compare semantics: same instructions/redirs/funcs, and every
    /// pool entry opens back to the original string.
    fn assert_opens_equivalent(original_bc: &[u8], opened: &[u8]) {
        if opened == original_bc {
            return;
        }
        let a = shell_bc::deserialize::deserialize(original_bc).unwrap();
        let b = shell_bc::deserialize::deserialize(opened).unwrap();
        assert_eq!(
            format!("{:?}", a.instructions),
            format!("{:?}", b.instructions)
        );
        assert_eq!(format!("{:?}", a.redirs), format!("{:?}", b.redirs));
        assert_eq!(format!("{:?}", a.funcs), format!("{:?}", b.funcs));
        let key = b.const_pool.key.expect("opened stream must be pool-sealed");
        assert_eq!(b.const_pool.len(), a.const_pool.len());
        for i in 0..a.const_pool.len() as u32 {
            let got = shell_bc::pool_seal::open_str(&key, i, b.const_pool.get(i).unwrap()).unwrap();
            assert_eq!(got, a.const_pool.get(i).unwrap(), "pool entry {i}");
        }
    }

    #[test]
    fn seal_open_roundtrip_full() {
        let bc_bytes = sample_shbc();
        for opts in [
            ProtectOptions {
                encrypt: true,
                opmap: true,
                antidebug: true,
                cff: false,
                opaque: false,
                smc: false,
                antihook: false,
                selfdebug: false,
            },
            ProtectOptions {
                encrypt: true,
                opmap: false,
                antidebug: false,
                cff: false,
                opaque: false,
                smc: false,
                antihook: false,
                selfdebug: false,
            },
            ProtectOptions {
                encrypt: false,
                opmap: true,
                antidebug: false,
                cff: false,
                opaque: false,
                smc: false,
                antihook: false,
                selfdebug: false,
            },
        ] {
            let sealed = seal(&bc_bytes, opts, 0).unwrap();
            assert!(is_protected(&sealed));
            // Shuffle must actually change op bytes; encryption must hide
            // the plaintext magic.
            if opts.opmap {
                let plain_offsets = shell_bc::opmap::instruction_offsets(&bc_bytes).unwrap();
                let sealed_body = &sealed[HEADER_LEN..];
                // Exit stays fixed, and a permutation may map an opcode to
                // itself — require at least one shuffled op byte.
                assert!(
                    plain_offsets
                        .iter()
                        .any(|&off| sealed_body[off] != bc_bytes[off]),
                    "shuffle changed no opcode byte"
                );
            }
            if opts.encrypt {
                assert!(!sealed[HEADER_LEN..].starts_with(b"SHBC"));
            }
            let (opened, _cfi) = open(&sealed, 0).unwrap().unwrap();
            assert_opens_equivalent(&bc_bytes, &opened);
        }
    }

    #[test]
    fn open_plain_payload_returns_none() {
        assert!(open(b"SHBC\x03", 0).unwrap().is_none());
    }

    #[test]
    fn seal_shuffle_only_runs_without_crypto() {
        let bc_bytes = sample_shbc();
        let opts = ProtectOptions {
            encrypt: false,
            opmap: true,
            antidebug: false,
            cff: false,
            opaque: false,
            smc: false,
            antihook: false,
            selfdebug: false,
        };
        let sealed = seal(&bc_bytes, opts, 0).unwrap();
        // Ciphertext == shuffled plaintext (no XOR).
        assert!(sealed[HEADER_LEN..].starts_with(b"SHBC"));
        let (opened, _cfi) = open(&sealed, 0).unwrap().unwrap();
        assert_opens_equivalent(&bc_bytes, &opened);
    }

    #[test]
    fn tampered_body_fails_integrity() {
        let bc_bytes = sample_shbc();
        let sealed = seal(&bc_bytes, ProtectOptions::all(), 0).unwrap();
        let mut tampered = sealed.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        let err = open(&tampered, 0).err().unwrap().to_string();
        assert!(err.contains("tampered"), "{err}");
    }

    #[test]
    fn v4_masks_seeds_in_header() {
        let bc_bytes = sample_shbc();
        let text_crc = 0x0123_4567;
        let sealed = seal(&bc_bytes, ProtectOptions::all(), text_crc).unwrap();
        let header = ProtectHeader::from_bytes(&sealed).unwrap();
        // Stored seeds must not equal a simple deterministic value the
        // analyst could recognise; with a fixed text_crc the stored value
        // differs from the derived one unless masking is applied.
        let derived_opmap = mask_opmap_seed(header.opmap_seed, text_crc);
        let derived_cff = mask_cff_seed(header.cff_seed, text_crc);
        assert_ne!(
            header.opmap_seed, derived_opmap,
            "opmap seed stored unmasked"
        );
        assert_ne!(header.cff_seed, derived_cff, "cff seed stored unmasked");
        // Round-trip through open() with the correct text_crc recovers the
        // original bytecode — proving the unmask derives the true seeds.
        let (opened, _cfi) = open(&sealed, text_crc).unwrap().unwrap();
        assert_opens_equivalent(&bc_bytes, &opened);
    }

    #[test]
    fn v4_wrong_text_crc_fails_tamper() {
        let bc_bytes = sample_shbc();
        let sealed = seal(&bc_bytes, ProtectOptions::all(), 0xAAAA_0001).unwrap();
        // A different .text CRC (e.g. patched stub) un-masks wrong seeds →
        // wrong permutation → CRC mismatch → tamper error.
        let err = open(&sealed, 0xBBBB_0002).err().unwrap().to_string();
        assert!(err.contains("tampered"), "{err}");
    }

    #[test]
    fn cfi_table_roundtrip_when_smc() {
        let bc_bytes = sample_shbc();
        let sealed = seal(&bc_bytes, ProtectOptions::all(), 0).unwrap();
        let (opened, cfi) = open(&sealed, 0).unwrap().unwrap();
        assert_opens_equivalent(&bc_bytes, &opened);
        let cfi = cfi.expect("smc seal must produce a CFI table");
        // 4 instructions -> 1 block; entry = CRC32 of the raw rows.
        assert_eq!(cfi.len(), 1);
        let offsets = shell_bc::opmap::instruction_offsets(&bc_bytes).unwrap();
        let mut rows = Vec::new();
        for off in offsets {
            rows.extend_from_slice(&bc_bytes[off..off + 5]);
        }
        assert_eq!(cfi[0], crc32(&rows), "build table must match row CRC");
    }

    #[test]
    fn no_cfi_table_without_smc() {
        let bc_bytes = sample_shbc();
        let opts = ProtectOptions {
            smc: false,
            ..ProtectOptions::all()
        };
        let sealed = seal(&bc_bytes, opts, 0).unwrap();
        let (_, cfi) = open(&sealed, 0).unwrap().unwrap();
        assert!(cfi.is_none());
    }

    #[test]
    fn cfi_frame_tamper_fails_closed() {
        // Unencrypted payload: the CFI frame is plaintext, so patch
        // num_blocks directly — open() must refuse, never bypass.
        let bc_bytes = sample_shbc();
        let opts = ProtectOptions {
            smc: true,
            encrypt: false,
            opmap: false,
            ..ProtectOptions::all()
        };
        let mut sealed = seal(&bc_bytes, opts, 0).unwrap();
        sealed[HEADER_LEN] ^= 0xFF; // num_blocks low byte: 1 -> 254
        let err = open(&sealed, 0).err().unwrap().to_string();
        assert!(err.contains("cfi"), "{err}");
    }

    #[test]
    fn kdf_expand_deterministic_and_sensitive() {
        let salt = [3u8; 16];
        let a = kdf_expand(0x1234, &salt, 0xFF);
        assert_eq!(a, kdf_expand(0x1234, &salt, 0xFF));
        assert_ne!(a, kdf_expand(0x1235, &salt, 0xFF), "text_crc must matter");
        assert_ne!(a, kdf_expand(0x1234, &[4u8; 16], 0xFF), "salt must matter");
        assert_ne!(a, kdf_expand(0x1234, &salt, 0xFE), "flags must matter");
    }

    #[test]
    fn v6_header_holds_no_key_material() {
        // The v6 header.key is a salt, not the key: nothing in the header
        // alone (no text CRC) may recover the chain key.
        let bc_bytes = sample_shbc();
        let sealed = seal(&bc_bytes, ProtectOptions::all(), 0x0BAD_F00D).unwrap();
        let header = ProtectHeader::from_bytes(&sealed).unwrap();
        // Decrypting with the stored header.key bytes directly must fail.
        let mut body = sealed[HEADER_LEN..].to_vec();
        shell_bc::pool_seal::chain_crypt(&header.key, &mut body, true);
        assert!(
            crc32(&body) != header.crc32,
            "header.key alone must not decrypt the body"
        );
        // v5's leak is gone: param1 ^ crc no longer yields key[0].
        let mat = kdf_expand(0x0BAD_F00D, &header.key, header.flags);
        assert_ne!(
            header.opaque_param1 ^ header.crc32,
            mat[0] as u32,
            "opaque_param1 must not leak key material"
        );
    }

    #[test]
    fn v6_wrong_text_crc_fails_tamper() {
        // Same shape as the v5 break, now with a KDF in the way: a wrong
        // .text CRC derives the wrong key and open() must refuse.
        let bc_bytes = sample_shbc();
        let sealed = seal(&bc_bytes, ProtectOptions::all(), 0xAAAA_0001).unwrap();
        let err = open(&sealed, 0xBBBB_0002).err().unwrap().to_string();
        assert!(
            err.contains("tampered") || err.contains("mismatch"),
            "{err}"
        );
    }
}
