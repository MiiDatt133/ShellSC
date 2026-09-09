//! Runtime entropy: the per-process salt that masks SMC pool data.
//! Built from ASLR addresses, the pid, and the monotonic clock so each
//! live process gets an unpredictable value that never exists in the
//! packed `.sc` file.

/// Mix a u32 into the running entropy state (splitmix64 step).
fn mix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Gather unpredictable runtime values: a stack address (ASLR), the pid,
/// and clock nanos. On platforms where a source is unavailable it just
/// contributes 0 — the others still carry entropy.
pub fn runtime_salt() -> u32 {
    let mut acc: u64 = 0x243F_6A88_85A3_08D3;

    // ASLR: address of a local variable differs per run per process.
    let local: u8 = 0;
    let stack_addr = &local as *const u8 as usize as u64;
    acc ^= mix(stack_addr);

    // Process identity.
    acc ^= mix(std::process::id() as u64);

    // High-resolution time.
    if let Ok(t) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        acc ^= mix(t.as_nanos() as u64);
    }

    (mix(acc) >> 32) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn salt_is_nonzero_and_varies() {
        let a = runtime_salt();
        let b = runtime_salt();
        assert_ne!(a, 0);
        // Two calls a hair apart normally differ; identical values only on
        // a truly broken clock, which would be a bug worth noticing.
        assert!(a != b || runtime_salt() != b);
    }
}
