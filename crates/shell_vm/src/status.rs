#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExitStatus(pub i32);

impl ExitStatus {
    pub const OK: ExitStatus = ExitStatus(0);
    pub const FAIL: ExitStatus = ExitStatus(1);

    pub fn from_code(code: i32) -> Self {
        Self(code)
    }

    pub fn code(self) -> i32 {
        self.0
    }

    pub fn success(self) -> bool {
        self.0 == 0
    }

    pub fn failed(self) -> bool {
        !self.success()
    }
}

impl From<i32> for ExitStatus {
    fn from(v: i32) -> Self {
        Self(v)
    }
}

impl std::fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
