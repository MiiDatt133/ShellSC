#[derive(Debug)]
pub struct Frame {
    /// Instruction pointer — index into Bytecode.instructions.
    pub ip: usize,
}

impl Frame {
    pub fn new() -> Self {
        Self { ip: 0 }
    }
}

impl Default for Frame {
    fn default() -> Self {
        Self::new()
    }
}
