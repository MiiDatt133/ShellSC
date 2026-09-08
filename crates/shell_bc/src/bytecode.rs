use shell_ast::ShellError;
use shell_ir::IrChunk;

use crate::{
    assembler::Assembler, const_pool::ConstPool, deserialize, opcode::Opcode,
    sbc_emitter::SbcEmitter, sbc_parser::SbcParser, serialize,
};

#[derive(Debug, Clone)]
pub struct Instruction {
    pub op: Opcode,
    pub operand: u32,
}

impl Instruction {
    pub fn new(op: Opcode, operand: u32) -> Self {
        Self { op, operand }
    }
    pub fn no_operand(op: Opcode) -> Self {
        Self { op, operand: 0 }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirKind {
    Out = 1,
    Append = 2,
    In = 3,
    OutFd = 4,
    InFd = 5,
    HereDoc = 6,
    /// Quoted-delimiter heredoc: literal body, no expansion.
    HereDocLit = 7,
}

impl RedirKind {
    pub fn to_u8(&self) -> u8 {
        match self {
            Self::Out => 1,
            Self::Append => 2,
            Self::In => 3,
            Self::OutFd => 4,
            Self::InFd => 5,
            Self::HereDoc => 6,
            Self::HereDocLit => 7,
        }
    }
    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            1 => Some(Self::Out),
            2 => Some(Self::Append),
            3 => Some(Self::In),
            4 => Some(Self::OutFd),
            5 => Some(Self::InFd),
            6 => Some(Self::HereDoc),
            7 => Some(Self::HereDocLit),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub enum RedirTarget {
    File(u32),
    FilePath(String),
    Fd(u32),
    HereDoc(String),
}

#[derive(Debug, Clone)]
pub struct RedirEntry {
    pub kind: RedirKind,
    pub fd: u32,
    pub target: RedirTarget,
}

#[derive(Debug, Clone)]
pub struct FuncEntry {
    pub name: String,
    pub entry_ip: u32,
}

#[derive(Debug, Clone)]
pub struct Bytecode {
    pub instructions: Vec<Instruction>,
    pub const_pool: ConstPool,
    pub redirs: Vec<RedirEntry>,
    pub funcs: Vec<FuncEntry>,
}

impl Bytecode {
    pub fn new() -> Self {
        Self {
            instructions: vec![],
            const_pool: ConstPool::new(),
            redirs: vec![],
            funcs: vec![],
        }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        serialize::serialize(self)
    }

    pub fn from_bytes(data: &[u8]) -> Result<Self, ShellError> {
        deserialize::deserialize(data)
    }
}

impl Default for Bytecode {
    fn default() -> Self {
        Self::new()
    }
}

pub struct BcCompiler;

impl BcCompiler {
    pub fn new() -> Self {
        Self
    }

    pub fn compile(&self, chunk: &IrChunk) -> Result<Bytecode, ShellError> {
        let sbc = SbcEmitter::new().emit(chunk);
        let instrs = SbcParser::new(&sbc).parse()?;
        Assembler::new().assemble(instrs)
    }

    pub fn emit_sbc(&self, chunk: &IrChunk) -> String {
        SbcEmitter::new().emit(chunk)
    }
}

impl Default for BcCompiler {
    fn default() -> Self {
        Self::new()
    }
}
