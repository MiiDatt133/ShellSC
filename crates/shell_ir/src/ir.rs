use crate::builtin::BuiltinId;

pub type Label = usize;

#[derive(Debug, Clone, Default)]
pub struct IrChunk {
    pub ops: Vec<IrOp>,
}

impl IrChunk {
    pub fn new() -> Self {
        Self { ops: vec![] }
    }
    pub fn push(&mut self, op: IrOp) -> usize {
        let i = self.ops.len();
        self.ops.push(op);
        i
    }
    pub fn push_placeholder(&mut self, op: IrOp) -> usize {
        self.push(op)
    }
    pub fn patch(&mut self, idx: usize, target: Label) {
        match &mut self.ops[idx] {
            IrOp::JmpIfFail(t) | IrOp::JmpIfOk(t) | IrOp::Jmp(t) => *t = target,
            IrOp::PipeSubshellBegin(t) => *t = target,
            other => panic!("patch on non-jump: {:?}", other),
        }
    }
    pub fn here(&self) -> Label {
        self.ops.len()
    }
}

#[derive(Debug, Clone)]
pub enum IrOp {
    PushConst(String),
    PushVar(String),
    /// Push each positional parameter as its own stack slot.
    PushArgs,
    ConcatN(usize),
    SetVar(String),

    Builtin(BuiltinId, usize),

    CmdSubBegin,
    CmdSubEnd,

    ExecExternal(usize),
    ExecExternalBg(usize),

    FuncDef {
        name: String,
        entry: Label,
    },
    FuncReturn,

    ForSetup(usize),
    ForBind(String),
    ForEnd,

    CaseBegin,
    CaseMatch(String),
    CaseMatchDyn,
    CaseEnd,

    PipeStart(usize),
    PipeStage,
    PipeEnd,
    /// Begin a subshell/brace-group pipeline stage. Operand = Label after PipeSubshellEnd.
    /// In pipeline-collect mode: push Subshell stage, jump to end.
    /// In direct-run mode (e.g. as a standalone subshell): begin env isolation.
    PipeSubshellBegin(Label),
    /// End marker for a pipeline subshell stage.
    PipeSubshellEnd,

    Redirect(IrRedir),
    /// Redirect where the target path is on the value stack (e.g. `> $var`).
    /// Emitted after the word-evaluation code that pushes the path.
    RedirectDyn {
        kind: IrRedirKind,
        fd: u32,
    },

    Jmp(Label),
    JmpIfFail(Label),
    JmpIfOk(Label),

    StatusOk,
    StatusFail,
    StatusFlip,
    Label(String),
    Exit,

    /// Evaluate arithmetic expression template.
    /// The operand string may contain `$var` references which are
    /// expanded against the runtime environment before evaluation.
    ArithEvalStack,

    /// Bash unary test: pop one string, apply `op` (e.g. "-f", "-z"), set status.
    BashUnary(String),

    /// Bash binary test: pop right then left, apply `op`, set status.
    BashBinary(String),

    /// Save the current env on the subshell stack (begin of `(...)` subshell).
    SubshellBegin,
    /// Restore the saved env from the subshell stack (end of `(...)` subshell).
    SubshellEnd,

    RedirSave,
    RedirRestore,

    /// Pop one string from the value stack, expand it as a glob pattern, and
    /// push all matches (or the original string if no matches / no glob chars).
    /// Updates the VM's `glob_surplus` counter by `matches_pushed - 1`.
    GlobExpand,

    /// Runtime variable parameter expansion: ${var:-word}, ${#var}, etc.
    /// Stack protocol depends on op:
    ///   Length   → var value is looked up by name; no stack args consumed.
    ///   Others   → modifier string is on top of stack.
    VarExpand {
        var: String,
        op: shell_ast::ast::VarExpandOp,
    },

    /// `arr=(x y)` / `arr+=(x y)`: elements were pushed as stack slots
    /// (one per element); `count`/`append` describe how to consume them.
    /// `local` = `local arr=(…)` — write the innermost function frame.
    ArrayAssign {
        name: String,
        append: bool,
        count: usize,
        local: bool,
    },
    /// `arr[i]=v`: index string then value on the value stack.
    ArraySetIndex {
        name: String,
    },
}

#[derive(Debug, Clone)]
pub struct IrRedir {
    pub kind: IrRedirKind,
    pub fd: u32,
    pub target: IrRedirTarget,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IrRedirKind {
    Out,
    Append,
    In,
    OutFd,
    InFd,
    HereDoc,
    HereString,
    /// Quoted-delimiter heredoc: body is literal (no expansion).
    HereDocLit,
}

#[derive(Debug, Clone)]
pub enum IrRedirTarget {
    File(String),
    Fd(u32),
    HereDoc(String),
}
