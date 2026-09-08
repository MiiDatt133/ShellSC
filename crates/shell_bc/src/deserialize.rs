use shell_ast::ShellError;

use crate::{
    bytecode::{Bytecode, FuncEntry, Instruction, RedirEntry, RedirKind, RedirTarget},
    const_pool::ConstPool,
    opcode::Opcode,
};

pub fn deserialize(data: &[u8]) -> Result<Bytecode, ShellError> {
    let mut r = Reader::new(data);

    if r.read_bytes(4)? != b"SHBC" {
        return Err(ShellError::BytecodeError("bad magic".into()));
    }

    let version = r.read_u8()?;
    if version != 0x01 && version != 0x02 && version != 0x03 {
        return Err(ShellError::BytecodeError(format!(
            "unsupported bytecode version: {}",
            version
        )));
    }
    r.read_bytes(3)?;

    let pool_count = r.read_u32()? as usize;
    let mut strings = Vec::with_capacity(pool_count);
    for _ in 0..pool_count {
        let len = r.read_u32()? as usize;
        let bytes = r.read_bytes(len)?;
        strings.push(
            String::from_utf8(bytes.to_vec())
                .map_err(|e| ShellError::BytecodeError(format!("utf8: {}", e)))?,
        );
    }
    let const_pool = ConstPool { strings };

    let redir_count = r.read_u32()? as usize;
    let mut redirs = Vec::with_capacity(redir_count);
    for _ in 0..redir_count {
        let kind = RedirKind::from_u8(r.read_u8()?)
            .ok_or_else(|| ShellError::BytecodeError("unknown redir kind".into()))?;
        let fd = r.read_u32()?;
        let target_type = r.read_u8()?;
        let target = match target_type {
            1 => RedirTarget::File(r.read_u32()?),
            2 => RedirTarget::Fd(r.read_u32()?),
            3 => {
                let len = r.read_u32()? as usize;
                let bytes = r.read_bytes(len)?;
                RedirTarget::HereDoc(
                    String::from_utf8(bytes.to_vec())
                        .map_err(|e| ShellError::BytecodeError(format!("heredoc utf8: {}", e)))?,
                )
            }
            t => {
                return Err(ShellError::BytecodeError(format!(
                    "unknown redir target: {}",
                    t
                )))
            }
        };
        redirs.push(RedirEntry { kind, fd, target });
    }

    let mut funcs = vec![];
    if version >= 0x02 {
        let func_count = r.read_u32()? as usize;
        funcs = Vec::with_capacity(func_count);
        for _ in 0..func_count {
            let name_len = r.read_u32()? as usize;
            let name_bytes = r.read_bytes(name_len)?;
            let name = String::from_utf8(name_bytes.to_vec())
                .map_err(|e| ShellError::BytecodeError(format!("func name utf8: {}", e)))?;
            let entry_ip = r.read_u32()?;
            funcs.push(FuncEntry { name, entry_ip });
        }
    }

    let instr_count = r.read_u32()? as usize;
    let mut instructions = Vec::with_capacity(instr_count);
    for _ in 0..instr_count {
        let op = Opcode::from_u8(r.read_u8()?)
            .ok_or_else(|| ShellError::BytecodeError("unknown opcode".into()))?;
        let operand = r.read_u32()?;
        instructions.push(Instruction::new(op, operand));
    }

    Ok(Bytecode {
        instructions,
        const_pool,
        redirs,
        funcs,
    })
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], ShellError> {
        if self.remaining() < n {
            return Err(ShellError::BytecodeError(format!(
                "unexpected EOF at {} (need {}, have {})",
                self.pos,
                n,
                self.remaining()
            )));
        }
        let s = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn read_u8(&mut self) -> Result<u8, ShellError> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_u32(&mut self) -> Result<u32, ShellError> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }
}
