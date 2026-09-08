pub mod echo;
pub mod exit;
pub mod export;
pub mod printf;
pub mod read;
pub mod sleep;
pub mod test;
pub mod trap;
pub mod unset;

use crate::status::ExitStatus;

pub struct BuiltinResult {
    pub out: Vec<u8>,
    pub status: ExitStatus,
}

impl BuiltinResult {
    pub fn ok() -> Self {
        Self {
            out: vec![],
            status: ExitStatus::OK,
        }
    }
    pub fn fail() -> Self {
        Self {
            out: vec![],
            status: ExitStatus::FAIL,
        }
    }
    pub fn with_out(bytes: impl Into<Vec<u8>>, status: ExitStatus) -> Self {
        Self {
            out: bytes.into(),
            status,
        }
    }
}
