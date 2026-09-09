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
pub const PROTECT_VERSION: u8 = 3;

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
        if bytes[12] != PROTECT_VERSION {
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

// ── Key masking ──────────────────────────────────────────────────────────────

fn mask_key(key: &[u8; 16], text_hash: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, b) in key.iter().enumerate() {
        out[i] = b ^ (text_hash >> ((i % 4) * 8)) as u8;
    }
    out
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

/// Seal the plaintext bytecode into a protected `.shellsc` payload.
/// `text_hash` is the CRC32 of the ELF stub's .text section; when encrypting,
/// the key stored in the header is XOR-masked with this hash so the real key
/// only exists after the stub derives it at runtime from its own code bytes.
pub fn seal(bc_bytes: &[u8], opts: ProtectOptions, text_hash: u32) -> Result<Vec<u8>, ShellError> {
    let mut rng = Rng::from_entropy();
    let key = rng.key16();
    let seed = rng.next_u32();

    let mut body = bc_bytes.to_vec();

    if opts.opmap {
        let perm = opcode_permutation(seed);
        apply_opmap(&mut body, &perm)?;
    }

    let crc = crc32(bc_bytes);
    if opts.encrypt {
        xor_crypt(&key, &mut body);
    }

    let cff_seed = if opts.cff { rng.next_u64() as u16 } else { 0 };
    let opaque_p1 = if opts.opaque {
        crc ^ (key[0] as u32)
    } else {
        0
    };
    let opaque_p2 = if opts.opaque {
        (key[1] as u32).wrapping_mul(key[2] as u32)
    } else {
        0
    };

    // Mask the key with the ELF .text hash so it never appears in the file.
    let stored_key = mask_key(&key, text_hash);

    let header = ProtectHeader {
        flags: opts.flags(),
        cff_seed,
        key: stored_key,
        crc32: crc,
        orig_len: bc_bytes.len() as u32,
        opmap_seed: if opts.opmap { seed } else { 0 },
        opaque_param1: opaque_p1,
        opaque_param2: opaque_p2,
    };

    let mut out = Vec::with_capacity(HEADER_LEN + body.len());
    out.extend_from_slice(&header.to_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Open a protected payload: returns the decrypted, opcode-restored
/// bytecode bytes. `text_hash` is the CRC32 of the ELF stub's .text section,
/// used to unmask the key stored in the header (version >= 3).
pub fn open(payload: &[u8], text_hash: u32) -> Result<Option<Vec<u8>>, ShellError> {
    if !is_protected(payload) {
        return Ok(None);
    }
    let header = ProtectHeader::from_bytes(payload)?;
    let mut body = payload[HEADER_LEN..].to_vec();

    if header.flags & FLAG_ENCRYPTED != 0 {
        let version = payload[12];
        let real_key = if version >= KEYDERIVE_MIN_VERSION {
            mask_key(&header.key, text_hash)
        } else {
            header.key
        };
        xor_crypt(&real_key, &mut body);
    }

    if header.flags & FLAG_OPMAP != 0 && header.opmap_seed != 0 {
        let inv = opcode_inverse(header.opmap_seed);
        revert_opmap(&mut body, &inv)?;
    }

    if crc32(&body) != header.crc32 {
        return Err(ShellError::IoError(
            "bytecode integrity check failed".into(),
        ));
    }
    if body.len() != header.orig_len as usize {
        return Err(ShellError::IoError("bytecode length mismatch".into()));
    }
    Ok(Some(body))
}

/// Number of real opcodes to permute. Shuffle the contiguous 0x01..=0x2A
/// range (42 opcodes incl. PushArgs, ArrayAssign, ArraySetIndex); Exit at
/// 0xFF stays put because the VM treats it as the terminator.
pub const OPMAP_LO: u8 = 0x01;
pub const OPMAP_HI: u8 = 0x2B; // exclusive: covers 0x01..=0x2A (42 opcodes)
pub const OPMAP_SIZE: usize = (OPMAP_HI - OPMAP_LO) as usize;

/// Derive a deterministic permutation of opcode bytes from a seed.
/// Returns `perm[orig - OPMAP_LO] = shuffled_byte`.
pub fn opcode_permutation(seed: u32) -> [u8; OPMAP_SIZE] {
    let mut perm: [u8; OPMAP_SIZE] = (OPMAP_LO..OPMAP_HI)
        .collect::<Vec<u8>>()
        .try_into()
        .unwrap();
    // Fisher–Yates driven by the seed.
    let mut rng = Rng(seed as u64 | 1);
    let mut i = perm.len();
    while i > 1 {
        i -= 1;
        let j = (rng.next_u64() as usize) % (i + 1);
        perm.swap(i, j);
    }
    perm
}

/// Inverse of `opcode_permutation`: `inv[shuffled - OPMAP_LO] = orig_byte`.
pub fn opcode_inverse(seed: u32) -> [u8; OPMAP_SIZE] {
    let perm = opcode_permutation(seed);
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
            let perm = opcode_permutation(seed);
            let inv = opcode_inverse(seed);
            for i in 0..OPMAP_SIZE {
                let shuffled = perm[i];
                assert_eq!(inv[(shuffled - OPMAP_LO) as usize], OPMAP_LO + i as u8);
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
            let opened = open(&sealed, 0).unwrap().unwrap();
            assert_eq!(opened, bc_bytes);
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
        let opened = open(&sealed, 0).unwrap().unwrap();
        assert_eq!(opened, bc_bytes);
    }

    #[test]
    fn tampered_body_fails_integrity() {
        let bc_bytes = sample_shbc();
        let sealed = seal(&bc_bytes, ProtectOptions::all(), 0).unwrap();
        let mut tampered = sealed.clone();
        let last = tampered.len() - 1;
        tampered[last] ^= 0xFF;
        let err = open(&tampered, 0).err().unwrap().to_string();
        assert!(err.contains("integrity") || err.contains("opcode"), "{err}");
    }
}
