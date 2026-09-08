#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinId {
    Echo = 0x01,
    Printf = 0x02,
    Read = 0x03,
    Test = 0x04,
    Sleep = 0x05,
    Export = 0x06,
    Unset = 0x07,
    Exit = 0x08,
    True = 0x09,
    False = 0x0A,
    Local = 0x0B,    // local var=value — set var in current scope
    Colon = 0x0C,    // : [args…]       — null command, always succeeds
    Set = 0x0D,      // set [--] [args…] — set positional parameters
    Wait = 0x0E,     // wait [pid…]      — wait for background jobs
    Trap = 0x0F,     // trap [cmd] [sig…] — set signal handlers
    Return = 0x10,   // return [n] — function exit with status n
    CommandV = 0x11, // command -v name — print path/type if found
    Exec = 0x12,     // exec [cmd…] or exec N< file — apply redirects permanently
    Eval = 0x13,     // eval [args…] — parse+run the joined string
    Shift = 0x14,    // shift [n] — drop first n positionals
}

impl BuiltinId {
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Self::Echo),
            0x02 => Some(Self::Printf),
            0x03 => Some(Self::Read),
            0x04 => Some(Self::Test),
            0x05 => Some(Self::Sleep),
            0x06 => Some(Self::Export),
            0x07 => Some(Self::Unset),
            0x08 => Some(Self::Exit),
            0x09 => Some(Self::True),
            0x0A => Some(Self::False),
            0x0B => Some(Self::Local),
            0x0C => Some(Self::Colon),
            0x0D => Some(Self::Set),
            0x0E => Some(Self::Wait),
            0x0F => Some(Self::Trap),
            0x10 => Some(Self::Return),
            0x11 => Some(Self::CommandV),
            0x12 => Some(Self::Exec),
            0x13 => Some(Self::Eval),
            0x14 => Some(Self::Shift),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Echo => "echo",
            Self::Printf => "printf",
            Self::Read => "read",
            Self::Test => "test",
            Self::Sleep => "sleep",
            Self::Export => "export",
            Self::Unset => "unset",
            Self::Exit => "exit",
            Self::True => "true",
            Self::False => "false",
            Self::Local => "local",
            Self::Colon => ":",
            Self::Set => "set",
            Self::Wait => "wait",
            Self::Trap => "trap",
            Self::Return => "return",
            Self::CommandV => "command",
            Self::Exec => "exec",
            Self::Eval => "eval",
            Self::Shift => "shift",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        match s {
            "echo" => Some(Self::Echo),
            "printf" => Some(Self::Printf),
            "read" => Some(Self::Read),
            "test" | "[" => Some(Self::Test),
            "sleep" => Some(Self::Sleep),
            "export" => Some(Self::Export),
            "unset" => Some(Self::Unset),
            "exit" => Some(Self::Exit),
            "true" => Some(Self::True),
            "false" => Some(Self::False),
            "local" => Some(Self::Local),
            ":" => Some(Self::Colon),
            "set" => Some(Self::Set),
            "wait" => Some(Self::Wait),
            "trap" => Some(Self::Trap),
            "return" => Some(Self::Return),
            "command" => Some(Self::CommandV),
            "exec" => Some(Self::Exec),
            "eval" => Some(Self::Eval),
            "shift" => Some(Self::Shift),
            _ => None,
        }
    }

    pub fn encode(self, argc: usize) -> u32 {
        ((self as u32) << 16) | (argc as u32 & 0xFFFF)
    }

    pub fn decode(operand: u32) -> Option<(Self, usize)> {
        let id = Self::from_u8((operand >> 16) as u8)?;
        let argc = (operand & 0xFFFF) as usize;
        Some((id, argc))
    }
}
