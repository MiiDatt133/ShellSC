use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use shell_ast::ShellError;
use shell_bc::{assemble_sbc, compile_to_sbc, Bytecode};
use shell_ir::{opt, Lowerer};
use shell_lex::Lexer;
use shell_pack::Packer;
use shell_parse::Parser;

pub fn read_source(path: &Path) -> Result<String> {
    std::fs::read_to_string(path).with_context(|| format!("reading '{}'", path.display()))
}

pub fn sh_to_sbc(src: &str) -> Result<String> {
    let tokens = Lexer::new(src).tokenize().map_err(map_shell_err)?;
    let script = Parser::new(tokens).parse().map_err(map_shell_err)?;
    let chunk = Lowerer::new().lower(&script).map_err(map_shell_err)?;
    Ok(compile_to_sbc(&opt::optimize(chunk)))
}

pub fn sbc_to_bytecode(sbc: &str) -> Result<Bytecode> {
    assemble_sbc(sbc).map_err(map_shell_err)
}

pub fn full_pipeline(
    src: &str,
    protect: bool,
    smc: bool,
    selfdebug: bool,
) -> Result<(String, Vec<u8>, Vec<u8>)> {
    let sbc = sh_to_sbc(src)?;
    let bc = sbc_to_bytecode(&sbc)?;
    let bc_bytes = bc.to_bytes();
    let sc_bytes = if protect {
        let mut opts = shell_pack::ProtectOptions::all();
        opts.smc = smc;
        opts.selfdebug = selfdebug;
        Packer::new()
            .pack_protected(&bc_bytes, opts)
            .map_err(map_shell_err)?
    } else {
        Packer::new().pack(&bc_bytes).map_err(map_shell_err)?
    };
    Ok((sbc, bc_bytes, sc_bytes))
}

pub fn with_extension(path: &Path, ext: &str) -> PathBuf {
    path.with_extension(ext)
}

pub fn output_path(input: &Path, ext: &str, override_path: Option<&str>) -> PathBuf {
    match override_path {
        Some(p) => PathBuf::from(p),
        None => with_extension(input, ext),
    }
}

pub fn write_bytes(path: &Path, data: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating dirs for '{}'", path.display()))?;
        }
    }
    std::fs::write(path, data).with_context(|| format!("writing '{}'", path.display()))
}

pub fn write_text(path: &Path, text: &str) -> Result<()> {
    write_bytes(path, text.as_bytes())
}

pub fn make_executable(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(path)
            .with_context(|| format!("stat '{}'", path.display()))?
            .permissions();
        let mode = perms.mode() | 0o111;
        perms.set_mode(mode);
        std::fs::set_permissions(path, perms)
            .with_context(|| format!("chmod '{}'", path.display()))?;
    }
    Ok(())
}

pub fn map_shell_err(e: ShellError) -> anyhow::Error {
    anyhow::anyhow!("{}", e)
}
