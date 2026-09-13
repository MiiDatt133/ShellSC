// Per-build VM constants: when SHELLSC_VARIANT_SEED is set (stub-variant
// builds), derive the CFF guard constants and junk-op count from the seed
// so each build's machine code differs. Without the env var the defaults
// match the historical hardcoded values — normal builds are unaffected.
use std::env;
use std::fs;
use std::path::Path;

fn main() {
    println!("cargo:rerun-if-env-changed=SHELLSC_VARIANT_SEED");
    let seed = env::var("SHELLSC_VARIANT_SEED")
        .ok()
        .and_then(|s| s.parse::<u64>().ok());
    let out = env::var("OUT_DIR").unwrap();
    let src = match seed {
        Some(s) => gen_variant(s),
        None => "pub const CFF_PERM_MULT: u32 = 0x85EB_CA6B;\npub const CFF_VIRT_MULT: u32 = 0xC2B2_AE35;\npub const CFF_GUARD_CONST: u32 = 0x9E37;\npub const CFF_JUNK_OPS: u32 = 0;\n".to_string(),
    };
    fs::write(Path::new(&out).join("variant_constants.rs"), src).unwrap();
}

fn gen_variant(seed: u64) -> String {
    let mut x = seed | 1;
    let mut next = || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        x
    };
    // Odd multipliers stay bijections mod 2^32 — permutation derivation
    // (enable_cff) must remain valid for any seed.
    let perm_mult = (next() as u32) | 1;
    let virt_mult = (next() as u32) | 1;
    let guard = next() as u32;
    let junk_ops = (next() % 6) as u32;
    format!(
        "pub const CFF_PERM_MULT: u32 = {:#010X};\npub const CFF_VIRT_MULT: u32 = {:#010X};\npub const CFF_GUARD_CONST: u32 = {:#010X};\npub const CFF_JUNK_OPS: u32 = {};\n",
        perm_mult, virt_mult, guard, junk_ops
    )
}
