#[derive(Debug, Clone, Copy)]
pub struct ScLayout {
    pub stub_len: usize,
    pub bc_offset: usize,
    pub bc_len: usize,
    pub total_len: usize,
}

impl ScLayout {
    pub fn compute(stub_len: usize, bc_offset: usize, bc_len: usize) -> Self {
        Self {
            stub_len,
            bc_offset,
            bc_len,
            total_len: stub_len + bc_len,
        }
    }
}
