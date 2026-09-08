pub mod assembler;
pub mod bytecode;
pub mod const_pool;
pub mod deserialize;
pub mod opcode;
pub mod opmap;
pub mod sbc_emitter;
pub mod sbc_parser;
pub mod serialize;

pub use assembler::Assembler;
pub use bytecode::{BcCompiler, Bytecode};
pub use opcode::Opcode;
pub use sbc_emitter::SbcEmitter;
pub use sbc_parser::{SbcArg, SbcInstr, SbcParser};

pub fn assemble_sbc(sbc: &str) -> Result<Bytecode, shell_ast::ShellError> {
    let instrs = SbcParser::new(sbc).parse()?;
    Assembler::new().assemble(instrs)
}

pub fn compile_to_sbc(chunk: &shell_ir::IrChunk) -> String {
    SbcEmitter::new().emit(chunk)
}
