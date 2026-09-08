use crate::error::ShellError;
use crate::span::Span;

#[derive(Debug, Clone)]
pub struct Script {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

impl Script {
    pub fn new(stmts: Vec<Stmt>, span: Span) -> Self {
        Self { stmts, span }
    }
    pub fn empty() -> Self {
        Self {
            stmts: vec![],
            span: Span::dummy(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Command(PipelineStmt),
    And(Box<Stmt>, Box<Stmt>),
    Or(Box<Stmt>, Box<Stmt>),
    Background(Box<Stmt>),
    If(IfStmt),
    While(WhileStmt),
    For(ForStmt),
    Case(CaseStmt),
    Function(FunctionDef),
    /// `( stmts )` — runs in a subshell; variable changes are not visible to the caller.
    Subshell {
        stmts: Vec<Stmt>,
        redirects: Vec<Redirect>,
    },
    /// `{ stmts }` — runs in the current shell (no env isolation).
    BraceGroup {
        stmts: Vec<Stmt>,
        redirects: Vec<Redirect>,
    },
    BashCond(BashCondStmt),
    Negate(Box<Stmt>),
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Command(p) => p.span,
            Stmt::And(a, _) => a.span(),
            Stmt::Or(a, _) => a.span(),
            Stmt::Background(s) => s.span(),
            Stmt::If(i) => i.span,
            Stmt::While(w) => w.span,
            Stmt::For(f) => f.span,
            Stmt::Case(c) => c.span,
            Stmt::Function(f) => f.span,
            Stmt::Subshell { .. } => Span::dummy(),
            Stmt::BraceGroup { .. } => Span::dummy(),
            Stmt::BashCond(b) => b.span,
            Stmt::Negate(s) => s.span(),
        }
    }
}

// ── Bash conditional [[ ... ]] ────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct BashCondStmt {
    pub expr: BashCondExpr,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum BashCondExpr {
    /// `! expr`
    Not(Box<BashCondExpr>),
    /// `expr1 && expr2`
    And(Box<BashCondExpr>, Box<BashCondExpr>),
    /// `expr1 || expr2`
    Or(Box<BashCondExpr>, Box<BashCondExpr>),
    /// `-f arg`, `-z arg`, …
    Unary { op: String, arg: Word },
    /// `left = right`, `left =~ right`, `left -eq right`, …
    Binary { left: Word, op: String, right: Word },
}

// ── Pipeline / command ────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PipelineStmt {
    pub pipeline: Pipeline,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum PipelineArm {
    /// A simple command (the common case).
    Simple(Command),
    /// A subshell `( stmts )` used as a pipeline stage.
    Subshell(Vec<Stmt>, Vec<Redirect>),
    /// A brace-group `{ stmts }` used as a pipeline stage.
    BraceGroup(Vec<Stmt>, Vec<Redirect>),
}

#[derive(Debug, Clone)]
pub struct Pipeline {
    pub arms: Vec<PipelineArm>,
    pub span: Span,
}

impl Pipeline {
    pub fn single(cmd: Command) -> Self {
        let span = cmd.span;
        Self {
            arms: vec![PipelineArm::Simple(cmd)],
            span,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Assignment {
    pub name: String,
    pub value: Vec<WordPart>,
    pub span: Span,
    /// `arr=(x y)` — per-element parts (each inner Vec is one element).
    pub is_array: bool,
    /// `arr+=(x y)` — append instead of replace.
    pub append: bool,
    /// `arr[i]=v` — element index assignment; raw index expr ("0", "$i", …).
    pub index: Option<String>,
    /// Element words when `is_array`; `value` holds the same flattened.
    pub elements: Vec<Word>,
    /// `local name=(…)` — declare in the current function frame instead
    /// of the innermost owning frame (bash local semantics).
    pub local_decl: bool,
}

impl Assignment {
    pub fn scalar(name: String, value: Vec<WordPart>, span: Span) -> Self {
        Self {
            name,
            value,
            span,
            is_array: false,
            append: false,
            index: None,
            elements: vec![],
            local_decl: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Command {
    pub assigns: Vec<Assignment>,
    pub argv: Vec<Word>,
    pub redirects: Vec<Redirect>,
    pub span: Span,
    /// `local name=(…)` — array literals inside a `local`/`declare` command.
    /// Lowered to frame-local ArrayAssign before the builtin runs.
    pub local_arrays: Vec<Assignment>,
}

impl Command {
    pub fn new(
        assigns: Vec<Assignment>,
        argv: Vec<Word>,
        redirects: Vec<Redirect>,
        span: Span,
    ) -> Self {
        Self {
            assigns,
            argv,
            redirects,
            span,
            local_arrays: vec![],
        }
    }
}

#[derive(Debug, Clone)]
pub struct Word {
    pub parts: Vec<WordPart>,
    pub span: Span,
    /// Set when the word contains unquoted `*`, `?`, or `[` — triggers
    /// filename (glob) expansion at runtime in `ExecExternal` / `Builtin`.
    pub may_glob: bool,
}

impl Word {
    pub fn literal(s: impl Into<String>, span: Span) -> Self {
        Self {
            parts: vec![WordPart::Literal(s.into())],
            span,
            may_glob: false,
        }
    }

    pub fn as_literal(&self) -> Option<&str> {
        if self.parts.len() == 1 {
            if let WordPart::Literal(ref s) = self.parts[0] {
                return Some(s.as_str());
            }
        }
        None
    }
}

impl std::fmt::Display for Word {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for part in &self.parts {
            match part {
                WordPart::Literal(s) => write!(f, "{}", s)?,
                WordPart::Var(v) => write!(f, "${}", v)?,
                WordPart::CmdSub(_) => write!(f, "$(…)")?,
                WordPart::ArithSub(_) => write!(f, "$((…))")?,
                WordPart::VarExpand(e) => write!(f, "${{{}}}", e.var)?,
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum WordPart {
    Literal(String),
    Var(String),
    CmdSub(Vec<Stmt>),
    /// Raw arithmetic expression template, e.g. `"$x + 1 * $y"`.
    /// Variable references are expanded at runtime before evaluation.
    ArithSub(String),
    VarExpand(VarExpand),
}

#[derive(Debug, Clone)]
pub struct VarExpand {
    pub var: String,
    pub op: VarExpandOp,
    pub modifier: Option<Box<Word>>,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarExpandOp {
    Plain,
    Default,       // ${var:-word}
    Assign,        // ${var:=word}
    Error,         // ${var:?word}
    Alt,           // ${var:+word}
    Length,        // ${#var}
    TrimPrefix,    // ${var#pattern}
    TrimSuffix,    // ${var%pattern}
    TrimPrefixAll, // ${var##pattern}
    TrimSuffixAll, // ${var%%pattern}
    Substring,     // ${var:offset[:length]}
    /// `${arr[i]}` / `${arr[@]}` — index is "@", "*", digits, or "$var".
    /// `@`/`*` push one slot per element (word-splitting contexts).
    Index(String),
    /// `${arr[@]}` inside a word with adjacent parts: single joined slot
    /// (concat context can't take multi-slot pushes).
    IndexJoin(String),
    /// `${arr[i]:-word}` — element with a default when unset/empty.
    IndexDefault(String),
    /// `${#arr[@]}` — number of set elements.
    ArrayLength(String),
    // Colon-less forms: test UNSET only (empty counts as set).
    DefaultUnset, // ${var-word}
    AssignUnset,  // ${var=word}
    ErrorUnset,   // ${var?word}
    AltUnset,     // ${var+word}
}

#[derive(Debug, Clone)]
pub struct Redirect {
    pub kind: RedirectKind,
    pub fd: Option<u32>,
    pub target: RedirectTarget,
    /// Heredoc with a quoted delimiter (`<<'EOF'`): body stays literal,
    /// no variable expansion.
    pub no_expand: bool,
    pub span: Span,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RedirectKind {
    Out,
    Append,
    In,
    OutFd,
    InFd,
    HereDoc,
    HereDocStrip,
    HereString,
}

#[derive(Debug, Clone)]
pub enum RedirectTarget {
    File(Word),
    Fd(u32),
    HereDoc(String),
}

#[derive(Debug, Clone)]
pub struct IfStmt {
    pub condition: Vec<Stmt>,
    pub then_body: Vec<Stmt>,
    pub else_body: Option<Vec<Stmt>>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct WhileStmt {
    pub condition: Vec<Stmt>,
    pub body: Vec<Stmt>,
    pub negated: bool,
    pub redirects: Vec<Redirect>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct ForStmt {
    pub var: String,
    pub items: Vec<Word>,
    pub body: Vec<Stmt>,
    pub redirects: Vec<Redirect>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct CaseStmt {
    pub word: Word,
    pub arms: Vec<CaseArm>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct CaseArm {
    pub patterns: Vec<Word>,
    pub body: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FunctionDef {
    pub name: String,
    pub body: Vec<Stmt>,
    pub span: Span,
    /// Redirects after the closing `}` (e.g. `func() { … } >/dev/null 2>&1`).
    /// Applied each time the function is called.
    pub redirects: Vec<Redirect>,
}

pub fn stub_arith_sub(_span: Span) -> Result<WordPart, ShellError> {
    Err(ShellError::not_implemented("arithmetic substitution"))
}

pub fn stub_var_expand(_span: Span) -> Result<WordPart, ShellError> {
    Err(ShellError::not_implemented("variable expansion"))
}
