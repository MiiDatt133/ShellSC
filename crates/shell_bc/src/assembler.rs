use shell_ast::ShellError;
use shell_ir::BuiltinId;

use crate::{
    bytecode::{Bytecode, FuncEntry, Instruction, RedirTarget},
    opcode::Opcode,
    sbc_parser::{parse_redir_args, SbcArg, SbcInstr},
};

pub struct Assembler {
    bc: Bytecode,
    lineno: usize,
}

impl Assembler {
    pub fn new() -> Self {
        Self {
            bc: Bytecode::new(),
            lineno: 0,
        }
    }

    pub fn assemble(mut self, instrs: Vec<SbcInstr>) -> Result<Bytecode, ShellError> {
        for (i, instr) in instrs.iter().enumerate() {
            self.lineno = i + 1;
            self.assemble_instr(instr)?;
        }
        Ok(self.bc)
    }

    fn assemble_instr(&mut self, instr: &SbcInstr) -> Result<(), ShellError> {
        match instr.mnemonic.as_str() {
            "PushConst" => {
                let s = self.req_str(instr, 0)?;
                let i = self.bc.const_pool.intern(&s);
                self.push(Opcode::PushConst, i);
            }
            "PushVar" => {
                let s = self.req_str(instr, 0)?;
                let i = self.bc.const_pool.intern(&s);
                self.push(Opcode::PushVar, i);
            }
            "PushArgs" => {
                self.push_bare(Opcode::PushArgs);
            }
            "ConcatN" => {
                let n = self.req_uint(instr, 0)?;
                self.push(Opcode::ConcatN, n);
            }
            "SetVar" => {
                let s = self.req_str(instr, 0)?;
                let i = self.bc.const_pool.intern(&s);
                self.push(Opcode::SetVar, i);
            }

            "Builtin" => {
                let name = self.req_str(instr, 0)?;
                let id = BuiltinId::from_name(&name).ok_or_else(|| {
                    ShellError::BytecodeError(format!(
                        "line {}: unknown builtin '{}'",
                        self.lineno, name
                    ))
                })?;
                let argc = self.req_uint(instr, 1)?;
                self.push(Opcode::Builtin, id.encode(argc as usize));
            }

            "CmdSubBegin" => self.push_bare(Opcode::CmdSubBegin),
            "CmdSubEnd" => self.push_bare(Opcode::CmdSubEnd),

            "ExecExternal" => {
                let n = self.req_uint(instr, 0)?;
                self.push(Opcode::ExecExternal, n);
            }
            "ExecExternalBg" => {
                let n = self.req_uint(instr, 0)?;
                self.push(Opcode::ExecExternalBg, n);
            }

            "FuncDef" => {
                let name = self.req_str(instr, 0)?;
                let entry_ip = self.req_uint(instr, 1)?;
                let idx = self.bc.funcs.len() as u32;
                self.bc.funcs.push(FuncEntry { name, entry_ip });
                self.push(Opcode::FuncDef, idx);
            }
            "FuncReturn" => self.push_bare(Opcode::FuncReturn),

            "ForSetup" => {
                let n = self.req_uint(instr, 0)?;
                self.push(Opcode::ForSetup, n);
            }
            "ForBind" => {
                let s = self.req_str(instr, 0)?;
                let i = self.bc.const_pool.intern(&s);
                self.push(Opcode::ForBind, i);
            }
            "ForEnd" => self.push_bare(Opcode::ForEnd),

            "CaseBegin" => self.push_bare(Opcode::CaseBegin),
            "CaseMatch" => {
                let s = self.req_str(instr, 0)?;
                let i = self.bc.const_pool.intern(&s);
                self.push(Opcode::CaseMatch, i);
            }
            "CaseMatchDyn" => self.push_bare(Opcode::CaseMatchDyn),
            "CaseEnd" => self.push_bare(Opcode::CaseEnd),

            "PipeStart" => {
                let n = self.req_uint(instr, 0)?;
                self.push(Opcode::PipeStart, n);
            }
            "PipeStage" => self.push_bare(Opcode::PipeStage),
            "PipeEnd" => self.push_bare(Opcode::PipeEnd),

            "Redirect" => {
                let entry = parse_redir_args(&instr.args, self.lineno)?;
                let entry = match entry.target {
                    RedirTarget::FilePath(ref path) => {
                        let idx = self.bc.const_pool.intern(path);
                        crate::bytecode::RedirEntry {
                            kind: entry.kind,
                            fd: entry.fd,
                            target: RedirTarget::File(idx),
                        }
                    }
                    _ => entry,
                };
                let idx = self.bc.redirs.len() as u32;
                self.bc.redirs.push(entry);
                self.push(Opcode::Redirect, idx);
            }

            "Jmp" => {
                let t = self.req_uint(instr, 0)?;
                self.push(Opcode::Jmp, t);
            }
            "JmpIfFail" => {
                let t = self.req_uint(instr, 0)?;
                self.push(Opcode::JmpIfFail, t);
            }
            "JmpIfOk" => {
                let t = self.req_uint(instr, 0)?;
                self.push(Opcode::JmpIfOk, t);
            }
            "StatusOk" => self.push_bare(Opcode::StatusOk),
            "StatusFail" => self.push_bare(Opcode::StatusFail),
            "StatusFlip" => self.push_bare(Opcode::StatusFlip),
            "Exit" => self.push_bare(Opcode::Exit),

            // New opcodes
            "ArithEvalStack" => {
                self.push_bare(Opcode::ArithEvalStack);
            }
            "SubshellBegin" => {
                self.push_bare(Opcode::SubshellBegin);
            }
            "RedirSave" => self.push_bare(Opcode::RedirSave),
            "RedirRestore" => self.push_bare(Opcode::RedirRestore),
            "SubshellEnd" => {
                self.push_bare(Opcode::SubshellEnd);
            }
            "GlobExpand" => {
                self.push_bare(Opcode::GlobExpand);
            }
            "PipeSubshellBegin" => {
                let end_ip = self.req_uint(instr, 0)?;
                self.push(Opcode::PipeSubshellBegin, end_ip);
            }
            "PipeSubshellEnd" => {
                self.push_bare(Opcode::PipeSubshellEnd);
            }
            "VarExpand" => {
                let var_name = self.req_str(instr, 0)?;
                let op_byte = self.req_uint(instr, 1)?;
                let pool_idx = self.bc.const_pool.intern(&var_name);
                let operand = (op_byte << 24) | (pool_idx & 0x00FF_FFFF);
                self.bc
                    .instructions
                    .push(Instruction::new(Opcode::VarExpand, operand));
            }
            "DynRedir" => {
                let kind_byte = self.req_uint(instr, 0)?;
                let fd = self.req_uint(instr, 1)?;
                let operand = (kind_byte << 24) | (fd & 0x00FF_FFFF);
                self.push(Opcode::DynRedir, operand);
            }
            "BashUnary" => {
                let op = self.req_str(instr, 0)?;
                let idx = self.bc.const_pool.intern(&op);
                self.push(Opcode::BashUnary, idx);
            }
            "BashBinary" => {
                let op = self.req_str(instr, 0)?;
                let idx = self.bc.const_pool.intern(&op);
                self.push(Opcode::BashBinary, idx);
            }
            "ArrayAssign" => {
                let name = self.req_str(instr, 0)?;
                let append = self.req_uint(instr, 1)? & 1;
                let count = self.req_uint(instr, 2)?;
                let local = self.opt_uint(instr, 3)?.unwrap_or(0) & 1;
                if count > 0x3F {
                    return Err(ShellError::BytecodeError(format!(
                        "line {}: ArrayAssign count {} exceeds 63",
                        self.lineno, count
                    )));
                }
                let pool_idx = self.bc.const_pool.intern(&name);
                if pool_idx > 0x00FF_FFFF {
                    return Err(ShellError::BytecodeError(format!(
                        "line {}: ArrayAssign pool index too large",
                        self.lineno
                    )));
                }
                // [append:1][local:1][count:6][pool_idx:24] — bit-disjoint.
                let operand = (append << 31) | (local << 30) | (count << 24) | pool_idx;
                self.push(Opcode::ArrayAssign, operand);
            }
            "ArraySetIndex" => {
                let name = self.req_str(instr, 0)?;
                let idx = self.bc.const_pool.intern(&name);
                self.push(Opcode::ArraySetIndex, idx);
            }

            other => {
                return Err(ShellError::BytecodeError(format!(
                    "line {}: unknown mnemonic '{}'",
                    self.lineno, other
                )))
            }
        }
        Ok(())
    }

    fn push(&mut self, op: Opcode, operand: u32) {
        self.bc.instructions.push(Instruction::new(op, operand));
    }
    fn push_bare(&mut self, op: Opcode) {
        self.bc.instructions.push(Instruction::no_operand(op));
    }

    fn req_str(&self, instr: &SbcInstr, idx: usize) -> Result<String, ShellError> {
        match instr.args.get(idx) {
            Some(SbcArg::Str(s)) => Ok(s.clone()),
            Some(SbcArg::Uint(n)) => Ok(n.to_string()),
            None => Err(ShellError::BytecodeError(format!(
                "line {}: '{}' missing string arg {}",
                self.lineno, instr.mnemonic, idx
            ))),
        }
    }

    fn req_uint(&self, instr: &SbcInstr, idx: usize) -> Result<u32, ShellError> {
        match instr.args.get(idx) {
            Some(SbcArg::Uint(n)) => Ok(*n),
            Some(SbcArg::Str(s)) => s.parse::<u32>().map_err(|_| {
                ShellError::BytecodeError(format!(
                    "line {}: '{}' expected uint, got '{}'",
                    self.lineno, instr.mnemonic, s
                ))
            }),
            None => Err(ShellError::BytecodeError(format!(
                "line {}: '{}' missing uint arg {}",
                self.lineno, instr.mnemonic, idx
            ))),
        }
    }

    /// Optional uint arg — None when absent (older .sbc without the field).
    fn opt_uint(&self, instr: &SbcInstr, idx: usize) -> Result<Option<u32>, ShellError> {
        match instr.args.get(idx) {
            None => Ok(None),
            Some(SbcArg::Uint(n)) => Ok(Some(*n)),
            Some(SbcArg::Str(s)) => s.parse::<u32>().map(Some).map_err(|_| {
                ShellError::BytecodeError(format!(
                    "line {}: '{}' expected uint, got '{}'",
                    self.lineno, instr.mnemonic, s
                ))
            }),
        }
    }
}

impl Default for Assembler {
    fn default() -> Self {
        Self::new()
    }
}
