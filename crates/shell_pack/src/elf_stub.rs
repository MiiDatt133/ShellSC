use std::{env, fs, path::PathBuf};

use shell_ast::ShellError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElfMachine {
    Arm32,
    Aarch64,
    X86_64,
    X86,
    Unsupported(&'static str),
}

pub fn target_machine() -> ElfMachine {
    #[cfg(target_arch = "arm")]
    return ElfMachine::Arm32;

    #[cfg(target_arch = "aarch64")]
    return ElfMachine::Aarch64;

    #[cfg(target_arch = "x86_64")]
    return ElfMachine::X86_64;

    #[cfg(target_arch = "x86")]
    return ElfMachine::X86;

    #[cfg(not(any(
        target_arch = "arm",
        target_arch = "aarch64",
        target_arch = "x86_64",
        target_arch = "x86"
    )))]
    return ElfMachine::Unsupported(std::env::consts::ARCH);
}

pub fn stub_file_name() -> Option<&'static str> {
    match target_machine() {
        ElfMachine::Arm32 => Some("stub-armv7l.elf"),
        ElfMachine::Aarch64 => Some("stub-aarch64.elf"),
        ElfMachine::X86_64 => Some("stub-x86_64.elf"),
        ElfMachine::X86 => Some("stub-i686.elf"),
        ElfMachine::Unsupported(_) => None,
    }
}

pub fn candidate_stub_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();

    if let Some(p) = env::var_os("SHELLSC_STUB_PATH") {
        paths.push(PathBuf::from(p));
        return paths;
    }

    if let Ok(exe) = env::current_exe() {
        let exe = exe.canonicalize().unwrap_or(exe);
        if let Some(bin_dir) = exe.parent() {
            paths.push(bin_dir.join("shell_stub"));

            if let Some(name) = stub_file_name() {
                paths.push(bin_dir.join("stubs").join(name));
            }

            if let Some(target_dir) = bin_dir.parent() {
                for profile in &["debug", "release"] {
                    paths.push(target_dir.join(profile).join("shell_stub"));
                }

                if let Some(ws_root) = target_dir.parent() {
                    paths.push(ws_root.join("target").join("debug").join("shell_stub"));
                    paths.push(ws_root.join("target").join("release").join("shell_stub"));
                    if let Some(name) = stub_file_name() {
                        paths.push(ws_root.join("stubs").join(name));
                    }
                }
            }
        }
    }

    if let Ok(cwd) = env::current_dir() {
        paths.push(cwd.join("target").join("debug").join("shell_stub"));
        paths.push(cwd.join("target").join("release").join("shell_stub"));
        if let Some(name) = stub_file_name() {
            paths.push(cwd.join("stubs").join(name));
        }
    }

    if let Some(dir) = env::var_os("SHELLSC_STUB_DIR") {
        if let Some(name) = stub_file_name() {
            paths.push(PathBuf::from(dir).join(name));
        }
    }

    let compile_time_stubs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("stubs");
    if let Some(name) = stub_file_name() {
        paths.push(compile_time_stubs.join(name));
    }

    paths
}

pub fn validate_stub(bytes: &[u8]) -> Result<(), ShellError> {
    if bytes.len() < 20 {
        return Err(ShellError::IoError("stub too small".into()));
    }
    if &bytes[0..4] != b"\x7fELF" {
        return Err(ShellError::IoError("stub is not an ELF".into()));
    }
    match target_machine() {
        ElfMachine::Arm32 => {
            if bytes[4] != 1 {
                return Err(ShellError::IoError("stub: expected ELF32".into()));
            }
            if u16::from_le_bytes([bytes[18], bytes[19]]) != 40 {
                return Err(ShellError::IoError("stub: expected EM_ARM".into()));
            }
        }
        ElfMachine::Aarch64 => {
            if bytes[4] != 2 {
                return Err(ShellError::IoError("stub: expected ELF64".into()));
            }
            if u16::from_le_bytes([bytes[18], bytes[19]]) != 183 {
                return Err(ShellError::IoError("stub: expected EM_AARCH64".into()));
            }
        }
        ElfMachine::X86_64 => {
            if bytes[4] != 2 {
                return Err(ShellError::IoError("stub: expected ELF64".into()));
            }
            if u16::from_le_bytes([bytes[18], bytes[19]]) != 62 {
                return Err(ShellError::IoError("stub: expected EM_X86_64".into()));
            }
        }
        ElfMachine::X86 => {
            if bytes[4] != 1 {
                return Err(ShellError::IoError("stub: expected ELF32".into()));
            }
            if u16::from_le_bytes([bytes[18], bytes[19]]) != 3 {
                return Err(ShellError::IoError("stub: expected EM_386".into()));
            }
        }
        ElfMachine::Unsupported(_) => {}
    }
    Ok(())
}

pub fn stub_bytes() -> Result<Vec<u8>, ShellError> {
    let mut last_err: Option<String> = None;

    for path in candidate_stub_paths() {
        match fs::read(&path) {
            Ok(bytes) => match validate_stub(&bytes) {
                Ok(()) => return Ok(bytes),
                Err(e) => last_err = Some(format!("{}: {}", path.display(), e)),
            },
            Err(e) => last_err = Some(format!("{}: {}", path.display(), e)),
        }
    }

    Err(ShellError::IoError(format!(
        "could not find shell_stub binary for arch '{}'; \
         run `cargo build` first. last error: {}",
        std::env::consts::ARCH,
        last_err.unwrap_or_else(|| "no candidates".into()),
    )))
}
