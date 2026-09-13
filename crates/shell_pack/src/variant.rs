//! Per-build stub variants: rebuild shell_stub with SHELLSC_VARIANT_SEED set
//! so the CFF guard constants and junk-op count baked into the stub's machine
//! code differ on every protected build. A trace of build A's handlers does
//! not transfer to build B.
//!
//! The variant build uses a private target dir so it never poisons the
//! workspace's normal build cache. Each seed gets its own dir, which also
//! gives incremental-rebuild speed when the same seed is reused.

use std::path::{Path, PathBuf};
use std::process::Command;

use shell_ast::ShellError;

use crate::protect::Rng;

/// Build a per-seed stub variant and return the binary path.
/// `ws_root` is the workspace root (directory containing Cargo.toml).
pub fn build_variant_stub(ws_root: &Path, seed: u32) -> Result<PathBuf, ShellError> {
    let target_dir = ws_root.join(format!("target/shellsc-variant-{seed}"));
    std::fs::create_dir_all(&target_dir)
        .map_err(|e| ShellError::IoError(format!("variant target dir: {}", e)))?;

    let status = Command::new("cargo")
        .env("SHELLSC_VARIANT_SEED", seed.to_string())
        .env("CARGO_TARGET_DIR", &target_dir)
        .arg("build")
        .arg("--release")
        .arg("-p")
        .arg("shell_stub")
        .current_dir(ws_root)
        .status()
        .map_err(|e| ShellError::IoError(format!("spawn cargo: {}", e)))?;
    if !status.success() {
        return Err(ShellError::IoError(format!(
            "variant stub build failed (seed {seed})"
        )));
    }

    let stub = target_dir.join("release/shell_stub");
    if !stub.exists() {
        return Err(ShellError::IoError(format!(
            "variant stub not found at {}",
            stub.display()
        )));
    }
    Ok(stub)
}

/// A fresh variant seed from the same entropy source as the protect key.
pub fn variant_seed() -> u32 {
    Rng::from_entropy().next_u32()
}

/// Resolve the workspace root: the directory containing Cargo.toml with a
/// `[workspace]` table (or the parent of this crate's manifest dir).
pub fn workspace_root() -> Result<PathBuf, ShellError> {
    // shell_pack is always built from within the workspace; walk up from
    // its manifest dir until a Cargo.toml declares [workspace] or no parent
    // remains (the root manifest is the workspace by structure).
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    loop {
        let manifest = dir.join("Cargo.toml");
        if let Ok(text) = std::fs::read_to_string(&manifest) {
            if text.contains("[workspace]") {
                return Ok(dir);
            }
        }
        if !dir.pop() {
            return Err(ShellError::IoError("workspace root not found".into()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variant_seed_is_nonzero_and_varies() {
        let a = variant_seed();
        let b = variant_seed();
        assert_ne!(a, 0);
        // Two entropy draws may theoretically collide; retry a few times
        // before declaring failure.
        let mut differ = a != b;
        for _ in 0..4 {
            if differ {
                break;
            }
            differ = a != variant_seed();
        }
        assert!(differ, "entropy source looks stuck at {a}");
    }

    #[test]
    fn workspace_root_resolves() {
        let root = workspace_root().unwrap();
        assert!(root.join("crates/shell_stub").exists());
    }
}
