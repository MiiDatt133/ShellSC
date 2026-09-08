use shell_ir::{IrChunk, IrOp, IrRedirKind, IrRedirTarget};

pub struct SbcEmitter;

impl SbcEmitter {
    pub fn new() -> Self {
        Self
    }

    pub fn emit(&self, chunk: &IrChunk) -> String {
        let mut out = String::new();

        for op in &chunk.ops {
            self.emit_op(op, &mut out);
        }

        out
    }

    fn emit_op(&self, op: &IrOp, out: &mut String) {
        match op {
            IrOp::PushConst(s) => {
                out.push_str(&format!("PushConst {}\n", q(s)));
            }

            IrOp::PushVar(v) => {
                out.push_str(&format!("PushVar {}\n", q(v)));
            }

            IrOp::PushArgs => {
                out.push_str("PushArgs\n");
            }

            IrOp::ConcatN(n) => {
                out.push_str(&format!("ConcatN {}\n", n));
            }

            IrOp::SetVar(n) => {
                out.push_str(&format!("SetVar {}\n", q(n)));
            }

            IrOp::Builtin(id, argc) => {
                out.push_str(&format!("Builtin {} {}\n", id.name(), argc));
            }

            IrOp::CmdSubBegin => {
                out.push_str("CmdSubBegin\n");
            }

            IrOp::CmdSubEnd => {
                out.push_str("CmdSubEnd\n");
            }

            IrOp::ExecExternal(n) => {
                out.push_str(&format!("ExecExternal {}\n", n));
            }

            IrOp::ExecExternalBg(n) => {
                out.push_str(&format!("ExecExternalBg {}\n", n));
            }

            IrOp::FuncDef { name, entry } => {
                out.push_str(&format!("FuncDef {} {}\n", q(name), entry));
            }

            IrOp::FuncReturn => {
                out.push_str("FuncReturn\n");
            }

            IrOp::ForSetup(n) => {
                out.push_str(&format!("ForSetup {}\n", n));
            }

            IrOp::ForBind(v) => {
                out.push_str(&format!("ForBind {}\n", q(v)));
            }

            IrOp::ForEnd => {
                out.push_str("ForEnd\n");
            }

            IrOp::CaseBegin => {
                out.push_str("CaseBegin\n");
            }

            IrOp::CaseMatch(p) => {
                out.push_str(&format!("CaseMatch {}\n", q(p)));
            }

            IrOp::CaseMatchDyn => {
                out.push_str("CaseMatchDyn\n");
            }

            IrOp::CaseEnd => {
                out.push_str("CaseEnd\n");
            }

            IrOp::PipeStart(n) => {
                out.push_str(&format!("PipeStart {}\n", n));
            }

            IrOp::PipeStage => {
                out.push_str("PipeStage\n");
            }

            IrOp::PipeEnd => {
                out.push_str("PipeEnd\n");
            }

            IrOp::PipeSubshellBegin(lbl) => {
                out.push_str(&format!("PipeSubshellBegin {}\n", lbl));
            }

            IrOp::PipeSubshellEnd => {
                out.push_str("PipeSubshellEnd\n");
            }

            IrOp::Redirect(r) => {
                let kind = match r.kind {
                    IrRedirKind::Out => "Out",
                    IrRedirKind::Append => "Append",
                    IrRedirKind::In => "In",
                    IrRedirKind::OutFd => "OutFd",
                    IrRedirKind::InFd => "InFd",
                    IrRedirKind::HereDoc => "HereDoc",
                    IrRedirKind::HereString => "HereString",
                    IrRedirKind::HereDocLit => "HereDocLit",
                };

                let target = match &r.target {
                    IrRedirTarget::File(p) => format!("File {}", q(p)),
                    IrRedirTarget::Fd(n) => format!("Fd {}", n),
                    IrRedirTarget::HereDoc(b) => format!("HereDoc {}", q(b)),
                };

                out.push_str(&format!("Redirect {} {} {}\n", kind, r.fd, target));
            }

            IrOp::RedirectDyn { kind, fd } => {
                let k: u8 = match kind {
                    IrRedirKind::Out => 0,
                    IrRedirKind::Append => 1,
                    IrRedirKind::In => 2,
                    IrRedirKind::OutFd => 3,
                    IrRedirKind::InFd => 4,
                    IrRedirKind::HereDoc => 5,
                    IrRedirKind::HereString => 6,
                    IrRedirKind::HereDocLit => 5, // unreachable: heredocs are never dynamic
                };

                out.push_str(&format!("DynRedir {} {}\n", k, fd));
            }

            IrOp::Jmp(t) => {
                out.push_str(&format!("Jmp {}\n", t));
            }

            IrOp::JmpIfFail(t) => {
                out.push_str(&format!("JmpIfFail {}\n", t));
            }

            IrOp::JmpIfOk(t) => {
                out.push_str(&format!("JmpIfOk {}\n", t));
            }

            IrOp::StatusOk => {
                out.push_str("StatusOk\n");
            }

            IrOp::StatusFail => {
                out.push_str("StatusFail\n");
            }

            IrOp::StatusFlip => {
                out.push_str("StatusFlip\n");
            }

            IrOp::Label(n) => {
                out.push_str(&format!("; label {}\n", n));
            }

            IrOp::Exit => {
                out.push_str("Exit\n");
            }

            IrOp::ArithEvalStack => {
                out.push_str("ArithEvalStack\n");
            }

            IrOp::BashUnary(op) => {
                out.push_str(&format!("BashUnary {}\n", q(op)));
            }

            IrOp::BashBinary(op) => {
                out.push_str(&format!("BashBinary {}\n", q(op)));
            }

            IrOp::SubshellBegin => {
                out.push_str("SubshellBegin\n");
            }

            IrOp::RedirSave => {
                out.push_str("RedirSave\n");
            }

            IrOp::RedirRestore => {
                out.push_str("RedirRestore\n");
            }
            IrOp::SubshellEnd => {
                out.push_str("SubshellEnd\n");
            }

            IrOp::GlobExpand => {
                out.push_str("GlobExpand\n");
            }

            IrOp::VarExpand { var, op } => {
                let op_byte: u8 = match op {
                    shell_ast::ast::VarExpandOp::Default => 0,
                    shell_ast::ast::VarExpandOp::Assign => 1,
                    shell_ast::ast::VarExpandOp::Error => 2,
                    shell_ast::ast::VarExpandOp::Alt => 3,
                    shell_ast::ast::VarExpandOp::Length => 4,
                    shell_ast::ast::VarExpandOp::TrimPrefix => 5,
                    shell_ast::ast::VarExpandOp::TrimSuffix => 6,
                    shell_ast::ast::VarExpandOp::Plain => 7,
                    shell_ast::ast::VarExpandOp::TrimPrefixAll => 8,
                    shell_ast::ast::VarExpandOp::TrimSuffixAll => 9,
                    shell_ast::ast::VarExpandOp::Substring => 10,
                    shell_ast::ast::VarExpandOp::DefaultUnset => 11,
                    shell_ast::ast::VarExpandOp::AssignUnset => 12,
                    shell_ast::ast::VarExpandOp::ErrorUnset => 13,
                    shell_ast::ast::VarExpandOp::AltUnset => 14,
                    shell_ast::ast::VarExpandOp::Index(_) => 15,
                    shell_ast::ast::VarExpandOp::ArrayLength(_) => 16,
                    shell_ast::ast::VarExpandOp::IndexJoin(_) => 17,
                    shell_ast::ast::VarExpandOp::IndexDefault(_) => 18,
                };

                out.push_str(&format!("VarExpand {} {}\n", q(var), op_byte));
            }

            IrOp::ArrayAssign {
                name,
                append,
                count,
                local,
            } => {
                out.push_str(&format!(
                    "ArrayAssign {} {} {} {}\n",
                    q(name),
                    *append as u8,
                    count,
                    *local as u8
                ));
            }

            IrOp::ArraySetIndex { name } => {
                out.push_str(&format!("ArraySetIndex {}\n", q(name)));
            }
        }
    }
}

impl Default for SbcEmitter {
    fn default() -> Self {
        Self::new()
    }
}

fn q(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);

    out.push('"');

    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c => out.push(c),
        }
    }

    out.push('"');
    out
}
