//! Offsets of opcode bytes inside a serialized `SHBC` stream.
//!
//! Opcode shuffling (Phase 3 protection) must rewrite only instruction
//! opcodes — the const pool and redirect targets are arbitrary bytes that
//! can collide with opcode values. This module walks the stream layout
//! (identical to `deserialize`) and reports where each `[op:1][operand:4]`
//! instruction starts.

use shell_ast::ShellError;

#[cfg(test)]
use crate::bytecode::Bytecode;

use crate::opcode::Opcode;

/// Byte offset of each instruction's opcode byte within the stream.
pub fn instruction_offsets(data: &[u8]) -> Result<Vec<usize>, ShellError> {
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
    for _ in 0..pool_count {
        let len = r.read_u32()? as usize;
        r.read_bytes(len)?;
    }

    let redir_count = r.read_u32()? as usize;
    for _ in 0..redir_count {
        r.read_u8()?; // kind
        r.read_u32()?; // fd
        let target_tag = r.read_u8()?;
        match target_tag {
            1 | 2 => {
                r.read_u32()?;
            }
            3 => {
                let len = r.read_u32()? as usize;
                r.read_bytes(len)?;
            }
            other => {
                return Err(ShellError::BytecodeError(format!(
                    "bad redir target tag {}",
                    other
                )))
            }
        }
    }

    let func_count = r.read_u32()? as usize;
    for _ in 0..func_count {
        let len = r.read_u32()? as usize;
        r.read_bytes(len)?;
        r.read_u32()?; // entry_ip
    }

    let instr_count = r.read_u32()? as usize;
    let mut offsets = Vec::with_capacity(instr_count);
    for _ in 0..instr_count {
        offsets.push(r.pos);
        let op_byte = r.read_u8()?;
        // Validate it decodes; operands are always 4 bytes in this format.
        if Opcode::from_u8(op_byte).is_none() {
            return Err(ShellError::BytecodeError(format!(
                "unknown opcode {:#04x}",
                op_byte
            )));
        }
        r.read_u32()?;
    }

    Ok(offsets)
}

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_bytes(&mut self, n: usize) -> Result<&'a [u8], ShellError> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| ShellError::BytecodeError("offset overflow".into()))?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or_else(|| ShellError::BytecodeError("unexpected end of bytecode".into()))?;
        self.pos = end;
        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, ShellError> {
        Ok(self.read_bytes(1)?[0])
    }

    fn read_u32(&mut self) -> Result<u32, ShellError> {
        let b = self.read_bytes(4)?;
        Ok(u32::from_le_bytes(b.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_on_minimal_bytecode() {
        let mut bc = Bytecode::default();
        bc.const_pool.strings.push("hi".to_string());
        bc.instructions.push(crate::bytecode::Instruction {
            op: Opcode::Exit,
            operand: 0,
        });
        let bytes = crate::serialize::serialize(&bc);
        let offsets = instruction_offsets(&bytes).unwrap();
        assert_eq!(offsets.len(), 1);
        // The instruction opcode must sit exactly where reported.
        assert_eq!(bytes[offsets[0]], Opcode::Exit.to_u8());
    }

    #[test]
    fn every_offset_is_valid_opcode() {
        let mut bc = Bytecode::default();
        bc.const_pool.strings.push("s".to_string());
        // Const-pool string bytes deliberately collide with opcode values:
        // a bad walker that scans the whole stream would land inside them.
        bc.const_pool
            .strings
            .push("\u{1}\u{2}\u{3}\u{4}".to_string());
        for op in [
            Opcode::PushConst,
            Opcode::Jmp,
            Opcode::Builtin,
            Opcode::Exit,
        ] {
            bc.instructions.push(crate::bytecode::Instruction {
                op: op.clone(),
                operand: 7,
            });
        }
        let bytes = crate::serialize::serialize(&bc);
        let offsets = instruction_offsets(&bytes).unwrap();
        assert_eq!(offsets.len(), 4);
        for &off in &offsets {
            assert!(Opcode::from_u8(bytes[off]).is_some());
        }
    }
}
