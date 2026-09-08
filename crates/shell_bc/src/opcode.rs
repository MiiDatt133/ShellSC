#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Opcode {
    PushConst,
    PushVar,
    ConcatN,
    SetVar,
    Builtin,
    CmdSubBegin,
    CmdSubEnd,
    ExecExternal,
    ExecExternalBg,
    FuncDef,
    FuncReturn,
    ForSetup,
    ForBind,
    ForEnd,
    CaseBegin,
    CaseMatch,
    CaseMatchDyn,
    CaseEnd,
    PipeStart,
    PipeStage,
    PipeEnd,
    Redirect,
    Jmp,
    JmpIfFail,
    JmpIfOk,
    StatusOk,
    StatusFail,
    StatusFlip,
    Exit,
    /// Evaluate arithmetic expression (operand = const pool idx for template string).
    ArithEvalStack,
    /// Bash unary test (operand = const pool idx for operator, e.g. "-f").
    BashUnary,
    /// Bash binary test (operand = const pool idx for operator, e.g. "=~").
    BashBinary,
    /// Save current env for subshell (no operand).
    SubshellBegin,
    /// Restore saved env after subshell (no operand).
    SubshellEnd,
    /// Pop pattern, push glob matches, update glob_surplus (no operand).
    GlobExpand,
    /// Begin a pipeline subshell/brace-group stage. Operand = end_ip (after PipeSubshellEnd).
    PipeSubshellBegin,
    /// End marker for a pipeline subshell stage (no operand).
    PipeSubshellEnd,
    /// Variable parameter expansion: ${var:-word}, ${#var}, etc.
    /// Operand encoding: high byte = op code, low 3 bytes = const pool idx for var name.
    VarExpand,
    /// Dynamic redirect — pop path from stack. Operand: high byte=kind, low 3 bytes=fd.
    DynRedir,
    RedirSave,
    RedirRestore,
    /// Push each positional parameter ($1..$N) as separate stack slots.
    /// Used for `$@`/`$*` in for-lists and command argv. No operand.
    PushArgs,
    /// `arr=(x y)` / `arr+=(x y)`: pop `count` elements off the value
    /// stack. Operand: [append:1][count:8][pool_idx:23].
    ArrayAssign,
    /// `arr[i]=v`: pop value then index off the value stack.
    /// Operand = const pool idx of the array name.
    ArraySetIndex,
}

impl Opcode {
    pub fn to_u8(&self) -> u8 {
        match self {
            Opcode::PushConst => 0x01,
            Opcode::PushVar => 0x02,
            Opcode::ConcatN => 0x03,
            Opcode::SetVar => 0x04,
            Opcode::Builtin => 0x05,
            Opcode::CmdSubBegin => 0x06,
            Opcode::CmdSubEnd => 0x07,
            Opcode::ExecExternal => 0x08,
            Opcode::ExecExternalBg => 0x09,
            Opcode::FuncDef => 0x0A,
            Opcode::FuncReturn => 0x0B,
            Opcode::PipeStart => 0x0C,
            Opcode::PipeStage => 0x0D,
            Opcode::PipeEnd => 0x0E,
            Opcode::Redirect => 0x0F,
            Opcode::Jmp => 0x10,
            Opcode::JmpIfFail => 0x11,
            Opcode::JmpIfOk => 0x12,
            Opcode::StatusOk => 0x13,
            Opcode::StatusFail => 0x14,
            Opcode::ForSetup => 0x15,
            Opcode::ForBind => 0x16,
            Opcode::ForEnd => 0x17,
            Opcode::CaseBegin => 0x18,
            Opcode::CaseMatch => 0x19,
            Opcode::CaseMatchDyn => 0x2B,
            Opcode::CaseEnd => 0x1A,
            Opcode::ArithEvalStack => 0x1B,
            Opcode::BashUnary => 0x1C,
            Opcode::BashBinary => 0x1D,
            Opcode::SubshellBegin => 0x1E,
            Opcode::SubshellEnd => 0x1F,
            Opcode::GlobExpand => 0x20,
            Opcode::PipeSubshellBegin => 0x21,
            Opcode::PipeSubshellEnd => 0x22,
            Opcode::VarExpand => 0x23,
            Opcode::DynRedir => 0x24,
            Opcode::RedirSave => 0x25,
            Opcode::RedirRestore => 0x26,
            Opcode::StatusFlip => 0x27,
            Opcode::PushArgs => 0x28,
            Opcode::ArrayAssign => 0x29,
            Opcode::ArraySetIndex => 0x2A,
            Opcode::Exit => 0xFF,
        }
    }

    pub fn from_u8(b: u8) -> Option<Self> {
        match b {
            0x01 => Some(Opcode::PushConst),
            0x02 => Some(Opcode::PushVar),
            0x03 => Some(Opcode::ConcatN),
            0x04 => Some(Opcode::SetVar),
            0x05 => Some(Opcode::Builtin),
            0x06 => Some(Opcode::CmdSubBegin),
            0x07 => Some(Opcode::CmdSubEnd),
            0x08 => Some(Opcode::ExecExternal),
            0x09 => Some(Opcode::ExecExternalBg),
            0x0A => Some(Opcode::FuncDef),
            0x0B => Some(Opcode::FuncReturn),
            0x0C => Some(Opcode::PipeStart),
            0x0D => Some(Opcode::PipeStage),
            0x0E => Some(Opcode::PipeEnd),
            0x0F => Some(Opcode::Redirect),
            0x10 => Some(Opcode::Jmp),
            0x11 => Some(Opcode::JmpIfFail),
            0x12 => Some(Opcode::JmpIfOk),
            0x13 => Some(Opcode::StatusOk),
            0x14 => Some(Opcode::StatusFail),
            0x15 => Some(Opcode::ForSetup),
            0x16 => Some(Opcode::ForBind),
            0x17 => Some(Opcode::ForEnd),
            0x18 => Some(Opcode::CaseBegin),
            0x19 => Some(Opcode::CaseMatch),
            0x1A => Some(Opcode::CaseEnd),
            0x1B => Some(Opcode::ArithEvalStack),
            0x1C => Some(Opcode::BashUnary),
            0x1D => Some(Opcode::BashBinary),
            0x1E => Some(Opcode::SubshellBegin),
            0x1F => Some(Opcode::SubshellEnd),
            0x20 => Some(Opcode::GlobExpand),
            0x21 => Some(Opcode::PipeSubshellBegin),
            0x22 => Some(Opcode::PipeSubshellEnd),
            0x23 => Some(Opcode::VarExpand),
            0x24 => Some(Opcode::DynRedir),
            0x25 => Some(Opcode::RedirSave),
            0x26 => Some(Opcode::RedirRestore),
            0x27 => Some(Opcode::StatusFlip),
            0x28 => Some(Opcode::PushArgs),
            0x29 => Some(Opcode::ArrayAssign),
            0x2A => Some(Opcode::ArraySetIndex),
            0x2B => Some(Opcode::CaseMatchDyn),
            0xFF => Some(Opcode::Exit),
            _ => None,
        }
    }

    pub fn has_operand(&self) -> bool {
        !matches!(
            self,
            Opcode::CmdSubBegin
                | Opcode::CmdSubEnd
                | Opcode::FuncReturn
                | Opcode::PipeStage
                | Opcode::PipeEnd
                | Opcode::ForEnd
                | Opcode::CaseBegin
                | Opcode::CaseMatchDyn
                | Opcode::CaseEnd
                | Opcode::StatusOk
                | Opcode::StatusFail
                | Opcode::Exit
                | Opcode::SubshellBegin
                | Opcode::SubshellEnd
                | Opcode::GlobExpand
                | Opcode::PipeSubshellEnd
                | Opcode::ArithEvalStack
                | Opcode::RedirSave
                | Opcode::RedirRestore
                | Opcode::PushArgs
        )
    }
}
