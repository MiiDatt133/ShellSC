use shell_ast::ShellError;

pub const MAGIC: &[u8; 16] = b"SHELLSC_FOOT_V01";
pub const TRAILER_SIZE: usize = 32;

#[derive(Debug, Clone, Copy)]
pub struct Trailer {
    pub bc_offset: u64,
    pub bc_len: u64,
}

impl Trailer {
    pub fn new(bc_offset: u64, bc_len: u64) -> Self {
        Self { bc_offset, bc_len }
    }

    pub fn to_bytes(&self) -> [u8; TRAILER_SIZE] {
        let mut buf = [0u8; TRAILER_SIZE];
        buf[0..8].copy_from_slice(&self.bc_offset.to_le_bytes());
        buf[8..16].copy_from_slice(&self.bc_len.to_le_bytes());
        buf[16..32].copy_from_slice(MAGIC);
        buf
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ShellError> {
        if bytes.len() < TRAILER_SIZE {
            return Err(ShellError::IoError("trailer too short".into()));
        }
        let tail = &bytes[bytes.len() - TRAILER_SIZE..];
        if &tail[16..32] != MAGIC {
            return Err(ShellError::IoError("bad .sc magic".into()));
        }
        let bc_offset = u64::from_le_bytes(tail[0..8].try_into().unwrap());
        let bc_len = u64::from_le_bytes(tail[8..16].try_into().unwrap());
        Ok(Self { bc_offset, bc_len })
    }

    pub fn extract_bc<'a>(&self, sc_bytes: &'a [u8]) -> Result<&'a [u8], ShellError> {
        let start = usize::try_from(self.bc_offset).map_err(|_| ShellError::IoError("bc_offset overflow".into()))?;
        let len = usize::try_from(self.bc_len).map_err(|_| ShellError::IoError("bc_len overflow".into()))?;
        let end = start.checked_add(len).ok_or_else(|| ShellError::IoError("bc region overflow".into()))?;
        let trailer_start = sc_bytes.len().checked_sub(TRAILER_SIZE).ok_or_else(|| ShellError::IoError("file too small".into()))?;
        if end != trailer_start {
            return Err(ShellError::IoError("bc region mismatch".into()));
        }
        Ok(&sc_bytes[start..end])
    }
}