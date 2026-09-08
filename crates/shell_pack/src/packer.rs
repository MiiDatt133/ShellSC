use goblin::elf::Elf;
use shell_ast::ShellError;

use crate::{
    elf_stub::{stub_bytes, validate_stub},
    layout::ScLayout,
    protect::{seal, ProtectOptions},
};

pub struct Packer;

const SECTION_NAME: &[u8] = b".shellsc\0";

impl Packer {
    pub fn new() -> Self {
        Self
    }

    pub fn pack(&self, bc_bytes: &[u8]) -> Result<Vec<u8>, ShellError> {
        let stub = stub_bytes()?;
        validate_stub(&stub)?;
        inject_section(&stub, bc_bytes)
    }

    /// Seal the bytecode (encrypt + opcode shuffle + CRC) and inject it
    /// as the `.shellsc` section. The stub reverses this at load time.
    pub fn pack_protected(
        &self,
        bc_bytes: &[u8],
        opts: ProtectOptions,
    ) -> Result<Vec<u8>, ShellError> {
        if !opts.is_any() {
            return self.pack(bc_bytes);
        }
        let sealed = seal(bc_bytes, opts)?;
        let stub = stub_bytes()?;
        validate_stub(&stub)?;
        inject_section(&stub, &sealed)
    }

    pub fn verify(sc_bytes: &[u8]) -> Result<ScLayout, ShellError> {
        let (offset, size) = find_shellsc_section(sc_bytes)?;
        Ok(ScLayout::compute(0, offset, size))
    }

    pub fn extract_bc(sc_bytes: &[u8]) -> Result<&[u8], ShellError> {
        let (offset, size) = find_shellsc_section(sc_bytes)?;
        sc_bytes
            .get(offset..offset + size)
            .ok_or_else(|| ShellError::IoError("shellsc section out of bounds".into()))
    }
}

impl Default for Packer {
    fn default() -> Self {
        Self::new()
    }
}

fn find_shellsc_section(bytes: &[u8]) -> Result<(usize, usize), ShellError> {
    let elf = Elf::parse(bytes).map_err(|e| ShellError::IoError(format!("goblin parse: {}", e)))?;

    for sh in &elf.section_headers {
        if elf.shdr_strtab.get_at(sh.sh_name) == Some(".shellsc") {
            let offset = sh.sh_offset as usize;
            let size = sh.sh_size as usize;
            return Ok((offset, size));
        }
    }

    Err(ShellError::IoError(
        "no .shellsc section found in .sc file".into(),
    ))
}

fn inject_section(stub: &[u8], bc_bytes: &[u8]) -> Result<Vec<u8>, ShellError> {
    let elf = Elf::parse(stub).map_err(|e| ShellError::IoError(format!("goblin parse: {}", e)))?;

    if elf.is_64 {
        inject64(stub, &elf, bc_bytes)
    } else {
        inject32(stub, &elf, bc_bytes)
    }
}

fn inject64(stub: &[u8], elf: &Elf, bc: &[u8]) -> Result<Vec<u8>, ShellError> {
    let e_shoff = elf.header.e_shoff as usize;
    let e_shentsize = elf.header.e_shentsize as usize;
    let e_shnum = elf.header.e_shnum as usize;
    let shstrndx = elf.header.e_shstrndx as usize;

    let strtab_sh = &elf.section_headers[shstrndx];
    let strtab_off = strtab_sh.sh_offset as usize;
    let strtab_sz = strtab_sh.sh_size as usize;
    let old_strtab = stub
        .get(strtab_off..strtab_off + strtab_sz)
        .ok_or_else(|| ShellError::IoError("shstrtab out of bounds".into()))?;

    let name_off = old_strtab.len() as u32;
    let mut new_strtab = old_strtab.to_vec();
    new_strtab.extend_from_slice(SECTION_NAME);

    let mut out = stub[..e_shoff].to_vec();

    let bc_offset = pad8(&mut out) as u64;
    out.extend_from_slice(bc);

    let strtab_offset = pad8(&mut out) as u64;
    out.extend_from_slice(&new_strtab);

    let new_shoff = pad8(&mut out) as u64;

    for i in 0..e_shnum {
        let start = e_shoff + i * e_shentsize;
        let end = start + e_shentsize;
        let raw = stub
            .get(start..end)
            .ok_or_else(|| ShellError::IoError("section header out of bounds".into()))?;
        let mut sh = raw.to_vec();
        if i == shstrndx {
            sh[24..32].copy_from_slice(&strtab_offset.to_le_bytes());
            sh[32..40].copy_from_slice(&(new_strtab.len() as u64).to_le_bytes());
        }
        out.extend_from_slice(&sh);
    }

    let mut sh = [0u8; 64];
    sh[0..4].copy_from_slice(&name_off.to_le_bytes());
    sh[4..8].copy_from_slice(&1u32.to_le_bytes());
    sh[8..16].copy_from_slice(&2u64.to_le_bytes());
    sh[24..32].copy_from_slice(&bc_offset.to_le_bytes());
    sh[32..40].copy_from_slice(&(bc.len() as u64).to_le_bytes());
    sh[48..56].copy_from_slice(&1u64.to_le_bytes());
    out.extend_from_slice(&sh);

    out[40..48].copy_from_slice(&new_shoff.to_le_bytes());
    let new_shnum = (e_shnum + 1) as u16;
    out[60..62].copy_from_slice(&new_shnum.to_le_bytes());

    Ok(out)
}

fn inject32(stub: &[u8], elf: &Elf, bc: &[u8]) -> Result<Vec<u8>, ShellError> {
    let e_shoff = elf.header.e_shoff as usize;
    let e_shentsize = elf.header.e_shentsize as usize;
    let e_shnum = elf.header.e_shnum as usize;
    let shstrndx = elf.header.e_shstrndx as usize;

    let strtab_sh = &elf.section_headers[shstrndx];
    let strtab_off = strtab_sh.sh_offset as usize;
    let strtab_sz = strtab_sh.sh_size as usize;
    let old_strtab = stub
        .get(strtab_off..strtab_off + strtab_sz)
        .ok_or_else(|| ShellError::IoError("shstrtab out of bounds".into()))?;

    let name_off = old_strtab.len() as u32;
    let mut new_strtab = old_strtab.to_vec();
    new_strtab.extend_from_slice(SECTION_NAME);

    let mut out = stub[..e_shoff].to_vec();

    let bc_offset = pad4(&mut out) as u32;
    out.extend_from_slice(bc);

    let strtab_offset = pad4(&mut out) as u32;
    out.extend_from_slice(&new_strtab);

    let new_shoff = pad4(&mut out) as u32;

    for i in 0..e_shnum {
        let start = e_shoff + i * e_shentsize;
        let end = start + e_shentsize;
        let raw = stub
            .get(start..end)
            .ok_or_else(|| ShellError::IoError("section header out of bounds".into()))?;
        let mut sh = raw.to_vec();
        if i == shstrndx {
            sh[16..20].copy_from_slice(&strtab_offset.to_le_bytes());
            sh[20..24].copy_from_slice(&(new_strtab.len() as u32).to_le_bytes());
        }
        out.extend_from_slice(&sh);
    }

    let mut sh = [0u8; 40];
    sh[0..4].copy_from_slice(&name_off.to_le_bytes());
    sh[4..8].copy_from_slice(&1u32.to_le_bytes());
    sh[8..12].copy_from_slice(&2u32.to_le_bytes());
    sh[16..20].copy_from_slice(&bc_offset.to_le_bytes());
    sh[20..24].copy_from_slice(&(bc.len() as u32).to_le_bytes());
    sh[32..36].copy_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&sh);

    out[32..36].copy_from_slice(&new_shoff.to_le_bytes());
    let new_shnum = (e_shnum + 1) as u16;
    out[48..50].copy_from_slice(&new_shnum.to_le_bytes());

    Ok(out)
}

fn pad8(buf: &mut Vec<u8>) -> usize {
    while buf.len() % 8 != 0 {
        buf.push(0);
    }
    buf.len()
}

fn pad4(buf: &mut Vec<u8>) -> usize {
    while buf.len() % 4 != 0 {
        buf.push(0);
    }
    buf.len()
}
