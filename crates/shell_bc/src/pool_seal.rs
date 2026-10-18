//! At-rest sealing shared by the packer (protect v5) and the VM's lazy
//! const-pool reads.
//!
//! Bytecode version 0x04 layout: after the 3 reserved bytes a 16-byte pool
//! key follows; every const-pool entry is
//! `hex(chain_crypt(entry_key(key, idx), utf8_bytes))`. `0x03` streams
//! (plain builds) are untouched.

use shell_ast::ShellError;

const PHI: u64 = 0x9E37_79B9_7F4A_7C15;

pub const POOL_SEAL_VERSION: u8 = 0x04;

pub fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Content-chained block keystream. Block `i`'s keystream mixes the
/// plaintext of block `i-1` back in together with fresh key material, so
/// known-plaintext cribbing unlocks only the cribbed block — deriving the
/// keystream for the next (unknown) block still needs the key.
///
/// `decrypt` only selects which side of the xor holds the plaintext for the
/// feedback read (encryptor reads it before the xor, decryptor after); the
/// state sequence is identical either way.
pub fn chain_crypt(key: &[u8; 16], data: &mut [u8], decrypt: bool) {
    let a = u64::from_le_bytes(key[0..8].try_into().unwrap());
    let b = u64::from_le_bytes(key[8..16].try_into().unwrap());
    let mut state = mix64(a ^ b.wrapping_mul(PHI));
    for (i, chunk) in data.chunks_mut(8).enumerate() {
        let ks = mix64(state ^ i as u64);
        let mut plain = 0u64;
        if decrypt {
            for (j, byte) in chunk.iter_mut().enumerate() {
                *byte ^= ((ks >> (8 * j)) & 0xFF) as u8;
            }
            for (j, &byte) in chunk.iter().enumerate() {
                plain |= (byte as u64) << (8 * j);
            }
        } else {
            for (j, &byte) in chunk.iter().enumerate() {
                plain |= (byte as u64) << (8 * j);
            }
            for (j, byte) in chunk.iter_mut().enumerate() {
                *byte ^= ((ks >> (8 * j)) & 0xFF) as u8;
            }
        }
        state = mix64(mix64(plain ^ a) ^ mix64(b.wrapping_add(i as u64)));
    }
}

/// Per-entry key: distinct keystream for every pool index.
fn entry_key(pool_key: &[u8; 16], idx: u32) -> [u8; 16] {
    let a = u64::from_le_bytes(pool_key[0..8].try_into().unwrap());
    let b = u64::from_le_bytes(pool_key[8..16].try_into().unwrap());
    let k0 = mix64(a ^ (idx as u64).wrapping_mul(PHI));
    let k1 = mix64(b ^ k0 ^ 0xD1B5_4A32_D192_ED03);
    let mut out = [0u8; 16];
    out[0..8].copy_from_slice(&k0.to_le_bytes());
    out[8..16].copy_from_slice(&k1.to_le_bytes());
    out
}

const HEX: &[u8; 16] = b"0123456789abcdef";

pub fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for &b in data {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 15) as usize] as char);
    }
    s
}

pub fn hex_decode(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(b.len() / 2);
    for pair in b.chunks(2) {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

/// Seal one pool entry: chain-encrypt then hex so the in-memory form is
/// ASCII garbage (invisible to `strings` on a heap dump).
pub fn seal_str(pool_key: &[u8; 16], idx: u32, s: &str) -> String {
    let mut bytes = s.as_bytes().to_vec();
    chain_crypt(&entry_key(pool_key, idx), &mut bytes, false);
    hex_encode(&bytes)
}

pub fn open_str(pool_key: &[u8; 16], idx: u32, hex: &str) -> Result<String, ShellError> {
    let mut bytes = hex_decode(hex)
        .ok_or_else(|| ShellError::BytecodeError("sealed pool entry: bad hex".into()))?;
    chain_crypt(&entry_key(pool_key, idx), &mut bytes, true);
    String::from_utf8(bytes)
        .map_err(|e| ShellError::BytecodeError(format!("sealed pool entry utf8: {}", e)))
}

/// Rewrite a serialized 0x03 stream as 0x04: splice `pool_key` after the
/// reserved bytes and seal every const-pool entry. Everything past the pool
/// (redirs, funcs, instructions) is copied verbatim.
pub fn seal_stream(bc_bytes: &[u8], pool_key: &[u8; 16]) -> Result<Vec<u8>, ShellError> {
    if bc_bytes.len() < 12 || &bc_bytes[0..4] != b"SHBC" {
        return Err(ShellError::BytecodeError("bad magic".into()));
    }
    if bc_bytes[4] != 0x03 {
        return Err(ShellError::BytecodeError(format!(
            "pool seal expects bytecode 0x03, got {:#04x}",
            bc_bytes[4]
        )));
    }
    let count = u32::from_le_bytes(bc_bytes[8..12].try_into().unwrap()) as usize;
    let mut pos = 12usize;
    let mut out = Vec::with_capacity(bc_bytes.len() + 16);
    out.extend_from_slice(b"SHBC");
    out.push(POOL_SEAL_VERSION);
    out.extend_from_slice(&[0, 0, 0]);
    out.extend_from_slice(pool_key);
    out.extend_from_slice(&(count as u32).to_le_bytes());
    for i in 0..count {
        if pos + 4 > bc_bytes.len() {
            return Err(ShellError::BytecodeError("pool truncated".into()));
        }
        let len = u32::from_le_bytes(bc_bytes[pos..pos + 4].try_into().unwrap()) as usize;
        pos += 4;
        if pos + len > bc_bytes.len() {
            return Err(ShellError::BytecodeError("pool truncated".into()));
        }
        let s = std::str::from_utf8(&bc_bytes[pos..pos + len])
            .map_err(|e| ShellError::BytecodeError(format!("pool utf8: {}", e)))?;
        pos += len;
        let sealed = seal_str(pool_key, i as u32, s);
        out.extend_from_slice(&(sealed.len() as u32).to_le_bytes());
        out.extend_from_slice(sealed.as_bytes());
    }
    out.extend_from_slice(&bc_bytes[pos..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chain_roundtrip() {
        let key = [7u8; 16];
        let mut data = b"SHBC\x03\x00\x00\x00hello world, padding to sixteen!".to_vec();
        let orig = data.clone();
        chain_crypt(&key, &mut data, false);
        assert_ne!(data, orig);
        chain_crypt(&key, &mut data, true);
        assert_eq!(data, orig);
    }

    #[test]
    fn chain_key_matters() {
        let mut a = vec![0u8; 64];
        let mut b = vec![0u8; 64];
        chain_crypt(&[1u8; 16], &mut a, false);
        chain_crypt(&[2u8; 16], &mut b, false);
        assert_ne!(a, b);
    }

    #[test]
    fn entry_roundtrip() {
        let key = [9u8; 16];
        let sealed = seal_str(&key, 3, "github_pat_abc");
        assert!(sealed.chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(open_str(&key, 3, &sealed).unwrap(), "github_pat_abc");
        assert!(open_str(&key, 4, &sealed).is_err(), "wrong idx must fail");
    }

    #[test]
    fn hex_roundtrip() {
        let data = [0u8, 1, 0xAB, 0xFF, 0x10];
        assert_eq!(hex_decode(&hex_encode(&data)).unwrap(), data);
        assert!(hex_decode("abc").is_none());
        assert!(hex_decode("zz").is_none());
    }
}
