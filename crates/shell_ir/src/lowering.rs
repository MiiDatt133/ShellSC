use crate::{
    builtin::BuiltinId,
    ir::{IrChunk, IrOp, IrRedir, IrRedirKind, IrRedirTarget, Label},
};
use shell_ast::{
    ast::{
        BashCondExpr, BashCondStmt, CaseStmt, Command, ForStmt, FunctionDef, IfStmt, Pipeline,
        PipelineArm, PipelineStmt, Redirect, RedirectKind, RedirectTarget, Script, Stmt, VarExpand,
        VarExpandOp, WhileStmt, Word, WordPart,
    },
    ShellError,
};

#[derive(Clone, Copy)]
enum LoopKind {
    While,
    For,
}

struct LoopCtx {
    kind: LoopKind,
    start: Label,
    break_holes: Vec<usize>,
}

pub struct Lowerer {
    chunk: IrChunk,
    loop_stack: Vec<LoopCtx>,
    /// Redirects desugared from `&>` / `&>>` — emitted right after the
    /// originating redirect by the lower_redirect caller.
    extra_redirs: Vec<IrRedir>,
}

impl Lowerer {
    pub fn new() -> Self {
        Self {
            chunk: IrChunk::new(),
            loop_stack: vec![],
            extra_redirs: vec![],
        }
    }

    pub fn lower(mut self, script: &Script) -> Result<IrChunk, ShellError> {
        for stmt in &script.stmts {
            self.lower_stmt(stmt)?;
        }
        self.chunk.push(IrOp::Exit);
        Ok(self.chunk)
    }

    fn lower_stmt(&mut self, stmt: &Stmt) -> Result<(), ShellError> {
        match stmt {
            Stmt::Command(p) => self.lower_pipeline_stmt(p),
            Stmt::And(a, b) => self.lower_and(a, b),
            Stmt::Or(a, b) => self.lower_or(a, b),
            Stmt::Background(s) => self.lower_background(s),
            Stmt::If(i) => self.lower_if(i),
            Stmt::While(w) => self.lower_while(w),
            Stmt::For(f) => self.lower_for(f),
            Stmt::Case(c) => self.lower_case(c),
            Stmt::Function(f) => self.lower_function(f),
            Stmt::Subshell { stmts, redirects } => self.lower_subshell(stmts, redirects),
            Stmt::BraceGroup { stmts, redirects } => self.lower_brace_group(stmts, redirects),
            Stmt::BashCond(b) => self.lower_bash_cond_stmt(b),
            Stmt::Negate(inner) => {
                self.lower_stmt(inner)?;
                self.chunk.push(IrOp::StatusFlip);
                Ok(())
            }
        }
    }

    fn lower_pipeline_stmt(&mut self, ps: &PipelineStmt) -> Result<(), ShellError> {
        self.lower_pipeline(&ps.pipeline)
    }

    fn lower_pipeline(&mut self, pl: &Pipeline) -> Result<(), ShellError> {
        let n = pl.arms.len();
        if n == 1 {
            match &pl.arms[0] {
                PipelineArm::Simple(cmd) => self.lower_command(cmd)?,
                PipelineArm::Subshell(stmts, redirects) => self.lower_subshell(stmts, redirects)?,
                PipelineArm::BraceGroup(stmts, redirects) => {
                    self.lower_brace_group(stmts, redirects)?
                }
            }
        } else {
            self.chunk.push(IrOp::PipeStart(n));
            for (i, arm) in pl.arms.iter().enumerate() {
                match arm {
                    PipelineArm::Simple(cmd) => self.lower_command(cmd)?,
                    PipelineArm::Subshell(stmts, redirects) => {
                        // Emit a "pipeline subshell" block with a forward jump.
                        let begin_hole = self.chunk.push_placeholder(IrOp::PipeSubshellBegin(0));
                        if !redirects.is_empty() {
                            self.chunk.push(IrOp::RedirSave);
                            for redir in redirects {
                                if let Some(ir) = self.lower_redirect(redir)? {
                                    self.chunk.push(IrOp::Redirect(ir));
                                }
                                for extra in self.extra_redirs.drain(..) {
                                    self.chunk.push(IrOp::Redirect(extra));
                                }
                            }
                        }
                        for s in stmts {
                            self.lower_stmt(s)?;
                        }
                        if !redirects.is_empty() {
                            self.chunk.push(IrOp::RedirRestore);
                        }
                        self.chunk.push(IrOp::PipeSubshellEnd);
                        let end = self.chunk.here();
                        self.chunk.patch(begin_hole, end);
                    }
                    PipelineArm::BraceGroup(stmts, redirects) => {
                        // Brace-group in a pipeline: same wire protocol as subshell
                        // (capture stdin/stdout) but no env isolation at runtime.
                        let begin_hole = self.chunk.push_placeholder(IrOp::PipeSubshellBegin(0));
                        if !redirects.is_empty() {
                            self.chunk.push(IrOp::RedirSave);
                            for redir in redirects {
                                if let Some(ir) = self.lower_redirect(redir)? {
                                    self.chunk.push(IrOp::Redirect(ir));
                                }
                                for extra in self.extra_redirs.drain(..) {
                                    self.chunk.push(IrOp::Redirect(extra));
                                }
                            }
                        }
                        for s in stmts {
                            self.lower_stmt(s)?;
                        }
                        if !redirects.is_empty() {
                            self.chunk.push(IrOp::RedirRestore);
                        }
                        self.chunk.push(IrOp::PipeSubshellEnd);
                        let end = self.chunk.here();
                        self.chunk.patch(begin_hole, end);
                    }
                }
                if i + 1 < n {
                    self.chunk.push(IrOp::PipeStage);
                }
            }
            self.chunk.push(IrOp::PipeEnd);
        }
        Ok(())
    }

    fn lower_command(&mut self, cmd: &Command) -> Result<(), ShellError> {
        for redir in &cmd.redirects {
            if let Some(ir) = self.lower_redirect(redir)? {
                self.chunk.push(IrOp::Redirect(ir));
            }
            for extra in self.extra_redirs.drain(..) {
                self.chunk.push(IrOp::Redirect(extra));
            }
            for extra in self.extra_redirs.drain(..) {
                self.chunk.push(IrOp::Redirect(extra));
            }
            // If lower_redirect returned None it already emitted RedirectDyn + word code.
        }
        for assign in &cmd.assigns {
            if assign.is_array {
                for el in &assign.elements {
                    // lower_word: may_glob elements go through GlobExpand
                    // (glob + IFS field-splitting, tracked via glob_surplus).
                    self.lower_word(el)?;
                }
                self.chunk.push(IrOp::ArrayAssign {
                    name: assign.name.clone(),
                    append: assign.append,
                    count: assign.elements.len(),
                    local: false,
                });
            } else if assign.index.is_some() {
                let idx = assign.index.clone().unwrap();
                self.chunk.push(IrOp::PushConst(idx));
                self.lower_word_single_parts(&assign.value)?;
                self.chunk.push(IrOp::ArraySetIndex {
                    name: assign.name.clone(),
                });
            } else {
                self.lower_word_single_parts(&assign.value)?;
                self.chunk.push(IrOp::SetVar(assign.name.clone()));
            }
        }
        if !cmd.local_arrays.is_empty() {
            for assign in &cmd.local_arrays {
                for el in &assign.elements {
                    self.lower_word(el)?;
                }
                self.chunk.push(IrOp::ArrayAssign {
                    name: assign.name.clone(),
                    append: assign.append,
                    count: assign.elements.len(),
                    local: true,
                });
            }
        }
        if cmd.argv.is_empty() {
            return Ok(());
        }

        let cmd_name = cmd.argv[0].as_literal();
        match cmd_name.and_then(classify) {
            Some(Class::Builtin(id)) => self.lower_builtin(id, cmd)?,
            Some(Class::Break) => self.lower_break(),
            Some(Class::Continue) => self.lower_continue(),
            Some(Class::Return) => self.lower_return(cmd)?,
            None => {
                for word in &cmd.argv {
                    self.lower_word(word)?;
                }
                self.chunk.push(IrOp::ExecExternal(cmd.argv.len()));
            }
        }
        Ok(())
    }

    fn lower_builtin(&mut self, id: BuiltinId, cmd: &Command) -> Result<(), ShellError> {
        let args = &cmd.argv[1..];
        match id {
            BuiltinId::Echo
            | BuiltinId::Printf
            | BuiltinId::Read
            | BuiltinId::Test
            | BuiltinId::Unset
            | BuiltinId::Shift => {
                for arg in args {
                    self.lower_word(arg)?;
                }
                self.chunk.push(IrOp::Builtin(id, args.len()));
            }
            BuiltinId::Sleep | BuiltinId::Exit => {
                if !args.is_empty() {
                    self.lower_word(&args[0])?;
                } else {
                    self.chunk.push(IrOp::PushConst("0".into()));
                }
                self.chunk.push(IrOp::Builtin(id, 1));
            }
            BuiltinId::True | BuiltinId::False => {
                self.chunk.push(IrOp::Builtin(id, 0));
            }
            // : takes any number of args (all evaluated for side-effects, then discarded)
            BuiltinId::Colon => {
                for arg in args {
                    self.lower_word(arg)?;
                }
                self.chunk.push(IrOp::Builtin(id, args.len()));
            }
            // local var[=value] … — lower like plain arg list
            BuiltinId::Local => {
                for arg in args {
                    self.lower_word(arg)?;
                }
                self.chunk.push(IrOp::Builtin(id, args.len()));
            }
            // set [--] [args…] — set positional parameters; or set -e/-x etc.
            BuiltinId::Set => {
                for arg in args {
                    self.lower_word(arg)?;
                }
                self.chunk.push(IrOp::Builtin(id, args.len()));
            }
            // wait [pid…] — wait for background jobs
            BuiltinId::Wait | BuiltinId::Trap => {
                for arg in args {
                    self.lower_word(arg)?;
                }
                self.chunk.push(IrOp::Builtin(id, args.len()));
            }
            // return is routed through lower_return (Class::Return), so
            // this arm is unreachable; kept for exhaustiveness.
            BuiltinId::Return => {
                for arg in args {
                    self.lower_word(arg)?;
                }
                self.chunk.push(IrOp::Builtin(id, args.len()));
            }
            BuiltinId::CommandV => {
                for arg in args {
                    self.lower_word(arg)?;
                }
                self.chunk.push(IrOp::Builtin(id, args.len()));
            }
            // exec [cmd…] / exec N< file — args lowered like a plain arg list;
            // redirects arrive via Redirect ops before the Builtin op.
            BuiltinId::Exec => {
                for arg in args {
                    self.lower_word(arg)?;
                }
                self.chunk.push(IrOp::Builtin(id, args.len()));
            }
            // eval [args…] — args joined with spaces, compiled and spliced
            // into the running stream by the VM.
            BuiltinId::Eval => {
                for arg in args {
                    self.lower_word(arg)?;
                }
                self.chunk.push(IrOp::Builtin(id, args.len()));
            }
            BuiltinId::Export => {
                let mut pairs = 0usize;
                for arg in args {
                    if let Some(lit) = arg.as_literal() {
                        // Pre-split literal "NAME=value" at compile time.
                        if let Some(eq) = lit.find('=') {
                            self.chunk.push(IrOp::PushConst(lit[..eq].to_string()));
                            self.chunk.push(IrOp::PushConst(lit[eq + 1..].to_string()));
                        } else {
                            self.chunk.push(IrOp::PushConst(lit.to_string()));
                            self.chunk.push(IrOp::PushConst(String::new()));
                        }
                    } else {
                        // Dynamic arg: evaluate the full word (may be "NAME=value").
                        // Pass as (name=full_string, value=empty); export::run_pairs
                        // will re-split on '=' at runtime.
                        self.lower_word(arg)?;
                        self.chunk.push(IrOp::PushConst(String::new()));
                    }
                    pairs += 1;
                }
                self.chunk.push(IrOp::Builtin(BuiltinId::Export, pairs));
            }
        }
        Ok(())
    }

    fn lower_break(&mut self) {
        if let Some(ctx) = self.loop_stack.last_mut() {
            if matches!(ctx.kind, LoopKind::For) {
                self.chunk.push(IrOp::ForEnd);
            }
            let hole = self.chunk.push_placeholder(IrOp::Jmp(0));
            self.loop_stack.last_mut().unwrap().break_holes.push(hole);
        }
    }

    fn lower_continue(&mut self) {
        if let Some(ctx) = self.loop_stack.last() {
            let start = ctx.start;
            self.chunk.push(IrOp::Jmp(start));
        }
    }

    fn lower_return(&mut self, cmd: &Command) -> Result<(), ShellError> {
        // FuncReturn consumes no stack value; a bare `return` must push nothing.
        if let Some(arg) = cmd.argv.get(1) {
            self.lower_word(arg)?;
            self.chunk.push(IrOp::Builtin(BuiltinId::Return, 1));
        }
        self.chunk.push(IrOp::FuncReturn);
        Ok(())
    }

    fn lower_function(&mut self, f: &FunctionDef) -> Result<(), ShellError> {
        let jmp_hole = self.chunk.push_placeholder(IrOp::Jmp(0));
        let entry = self.chunk.here();
        if !f.redirects.is_empty() {
            self.chunk.push(IrOp::RedirSave);
            for redir in &f.redirects {
                if let Some(ir) = self.lower_redirect(redir)? {
                    self.chunk.push(IrOp::Redirect(ir));
                }
            }
        }
        for s in &f.body {
            self.lower_stmt(s)?;
        }
        if !f.redirects.is_empty() {
            self.chunk.push(IrOp::RedirRestore);
        }
        self.chunk.push(IrOp::FuncReturn);
        let after = self.chunk.here();
        self.chunk.patch(jmp_hole, after);
        self.chunk.push(IrOp::FuncDef {
            name: f.name.clone(),
            entry,
        });
        Ok(())
    }

    fn lower_word(&mut self, word: &Word) -> Result<(), ShellError> {
        self.lower_word_single_parts(&word.parts)?;
        if word.may_glob {
            self.chunk.push(IrOp::GlobExpand);
        }
        Ok(())
    }

    fn lower_word_single_parts(&mut self, parts: &[WordPart]) -> Result<(), ShellError> {
        let n = self.push_parts(parts)?;
        if n > 1 {
            self.chunk.push(IrOp::ConcatN(n));
        }
        Ok(())
    }

    fn push_parts(&mut self, parts: &[WordPart]) -> Result<usize, ShellError> {
        if parts.is_empty() {
            self.chunk.push(IrOp::PushConst(String::new()));
            return Ok(1);
        }
        let mut count = 0usize;
        let mut lit_buf = String::new();

        for part in parts {
            match part {
                WordPart::Literal(s) => lit_buf.push_str(s),
                WordPart::Var(v) => {
                    if !lit_buf.is_empty() {
                        self.chunk
                            .push(IrOp::PushConst(std::mem::take(&mut lit_buf)));
                        count += 1;
                    }
                    if v == "@" && parts.len() == 1 {
                        // `$@` as the whole word: push each positional as a
                        // separate slot so quoted "$@" keeps N words.
                        self.chunk.push(IrOp::PushArgs);
                        count += 1;
                    } else {
                        // `$*` stays one IFS-joined slot via the normal var
                        // expansion path (bash: "$*" is a single word).
                        self.chunk.push(IrOp::PushVar(v.clone()));
                        count += 1;
                    }
                }
                WordPart::CmdSub(stmts) => {
                    if !lit_buf.is_empty() {
                        self.chunk
                            .push(IrOp::PushConst(std::mem::take(&mut lit_buf)));
                        count += 1;
                    }
                    self.chunk.push(IrOp::CmdSubBegin);
                    for s in stmts {
                        self.lower_stmt(s)?;
                    }
                    self.chunk.push(IrOp::CmdSubEnd);
                    count += 1;
                }
                WordPart::ArithSub(expr) => {
                    // Flush pending literal first.
                    if !lit_buf.is_empty() {
                        self.chunk
                            .push(IrOp::PushConst(std::mem::take(&mut lit_buf)));
                        count += 1;
                    }
                    // Assignment forms (`n++`, `++n`, `n+=2`, …) write back to
                    // the variable — handled as a dedicated emit sequence.
                    if let Some(n) = self.lower_arith_assign(expr)? {
                        count += n;
                    } else {
                        // Expand $var references at compile time using PushVar so
                        // the well-tested variable-lookup path is reused.  The result
                        // is a string on the stack that ArithEvalStack then evaluates.
                        let arith_parts = parse_arith_expr_parts(expr);
                        let n = self.push_parts(&arith_parts)?;
                        if n > 1 {
                            self.chunk.push(IrOp::ConcatN(n));
                        }
                        self.chunk.push(IrOp::ArithEvalStack);
                        count += 1;
                    }
                }
                WordPart::VarExpand(e) => {
                    if !lit_buf.is_empty() {
                        self.chunk
                            .push(IrOp::PushConst(std::mem::take(&mut lit_buf)));
                        count += 1;
                    }
                    // `@`/`*` index with adjacent parts must collapse to a
                    // single slot (ConcatN can't absorb multi-slot pushes).
                    let op = if let VarExpandOp::Index(ref idx) = e.op {
                        if (idx == "@" || idx == "*") && parts.len() > 1 {
                            VarExpandOp::IndexJoin(idx.clone())
                        } else {
                            e.op.clone()
                        }
                    } else {
                        e.op.clone()
                    };
                    match &op {
                        VarExpandOp::Length | VarExpandOp::Plain => {
                            // No modifier needed on stack.
                        }
                        VarExpandOp::Index(ref idx)
                        | VarExpandOp::IndexJoin(ref idx)
                        | VarExpandOp::ArrayLength(ref idx) => {
                            self.chunk.push(IrOp::PushConst(idx.clone()));
                        }
                        // `${arr[i]:-word}` — stack: [index, modifier].
                        VarExpandOp::IndexDefault(ref idx) => {
                            self.chunk.push(IrOp::PushConst(idx.clone()));
                            if let Some(ref modifier) = e.modifier {
                                self.lower_word_single_parts(&modifier.parts)?;
                            } else {
                                self.chunk.push(IrOp::PushConst(String::new()));
                            }
                        }
                        _ => {
                            if let Some(ref modifier) = e.modifier {
                                self.lower_word_single_parts(&modifier.parts)?;
                            } else {
                                self.chunk.push(IrOp::PushConst(String::new()));
                            }
                        }
                    }
                    self.chunk.push(IrOp::VarExpand {
                        var: e.var.clone(),
                        op,
                    });
                    count += 1;
                }
            }
        }

        if !lit_buf.is_empty() {
            self.chunk.push(IrOp::PushConst(lit_buf));
            count += 1;
        }
        if count == 0 {
            self.chunk.push(IrOp::PushConst(String::new()));
            count = 1;
        }
        Ok(count)
    }

    /// Lower a word that must evaluate to ONE stack slot (e.g. `<<<` target):
    /// multi-slot `${arr[@]}` collapses to a single space-joined slot.
    fn lower_word_joined(&mut self, word: &Word) -> Result<(), ShellError> {
        let parts: Vec<WordPart> = word
            .parts
            .iter()
            .map(|p| match p {
                WordPart::VarExpand(e) => {
                    let mut e = e.clone();
                    if let VarExpandOp::Index(idx) = &e.op {
                        if idx == "@" {
                            e.op = VarExpandOp::IndexJoin(idx.clone());
                        }
                    }
                    WordPart::VarExpand(e)
                }
                other => other.clone(),
            })
            .collect();
        self.lower_word_single_parts(&parts)
    }

    fn lower_redirect(&mut self, redir: &Redirect) -> Result<Option<IrRedir>, ShellError> {
        // `&>file` / `&>>file` — desugar into `>file 2>&1` / `>>file 2>&1`:
        // fd1 to the file, then fd2 duplicating fd1. The second redirect is
        // queued in extra_redirs for the caller to emit right after this one.
        if matches!(redir.kind, RedirectKind::Both | RedirectKind::BothAppend) {
            let base_kind = if redir.kind == RedirectKind::Both {
                IrRedirKind::Out
            } else {
                IrRedirKind::Append
            };
            if let RedirectTarget::File(w) = &redir.target {
                if let Some(lit) = w.as_literal() {
                    let file = lit.to_string();
                    self.extra_redirs.push(IrRedir {
                        kind: IrRedirKind::OutFd,
                        fd: 2,
                        target: IrRedirTarget::Fd(1),
                    });
                    return Ok(Some(IrRedir {
                        kind: base_kind,
                        fd: 1,
                        target: IrRedirTarget::File(file),
                    }));
                }
                // Dynamic target: evaluate word, RedirectDyn for fd1, then
                // dup fd2→fd1. RedirectDyn pops its target from the stack, so
                // lower the word twice.
                self.lower_word(w)?;
                self.chunk.push(IrOp::RedirectDyn {
                    kind: base_kind,
                    fd: 1,
                });
                self.extra_redirs.push(IrRedir {
                    kind: IrRedirKind::OutFd,
                    fd: 2,
                    target: IrRedirTarget::Fd(1),
                });
                return Ok(None);
            }
        }
        let kind = match redir.kind {
            RedirectKind::Out => IrRedirKind::Out,
            RedirectKind::Append => IrRedirKind::Append,
            RedirectKind::In => IrRedirKind::In,
            RedirectKind::OutFd => IrRedirKind::OutFd,
            RedirectKind::InFd => IrRedirKind::InFd,
            // Both/BothAppend are desugared and return early above.
            RedirectKind::Both | RedirectKind::BothAppend => IrRedirKind::Out,
            RedirectKind::HereDoc | RedirectKind::HereDocStrip => {
                if redir.no_expand {
                    IrRedirKind::HereDocLit
                } else {
                    IrRedirKind::HereDoc
                }
            }
            RedirectKind::HereString => IrRedirKind::HereString,
        };
        let fd = match redir.kind {
            RedirectKind::In
            | RedirectKind::InFd
            | RedirectKind::HereDoc
            | RedirectKind::HereDocStrip
            | RedirectKind::HereString => redir.fd.unwrap_or(0),
            _ => redir.fd.unwrap_or(1),
        };
        let target = match &redir.target {
            RedirectTarget::File(w) => {
                if matches!(redir.kind, RedirectKind::HereString) {
                    // <<< target is a single string: collapse multi-slot
                    // expansions (`"${a[@]}"`) to one space-joined slot.
                    self.lower_word_joined(w)?;
                    self.chunk.push(IrOp::RedirectDyn { kind, fd });
                    return Ok(None);
                }
                if let Some(lit) = w.as_literal() {
                    IrRedirTarget::File(lit.to_string())
                } else {
                    // Dynamic target: evaluate the word onto the stack, then emit RedirectDyn.
                    self.lower_word(w)?;
                    self.chunk.push(IrOp::RedirectDyn { kind, fd });
                    return Ok(None);
                }
            }
            RedirectTarget::Fd(n) => IrRedirTarget::Fd(*n),
            RedirectTarget::HereDoc(body) => {
                let body = if matches!(redir.kind, RedirectKind::HereDocStrip) {
                    Self::strip_heredoc_tabs(body)
                } else {
                    body.clone()
                };
                IrRedirTarget::HereDoc(body)
            }
        };
        Ok(Some(IrRedir { kind, fd, target }))
    }

    fn strip_heredoc_tabs(body: &str) -> String {
        let mut out = String::new();
        for seg in body.split_inclusive('\n') {
            if let Some(line) = seg.strip_suffix('\n') {
                out.push_str(line.trim_start_matches('\t'));
                out.push('\n');
            } else {
                out.push_str(seg.trim_start_matches('\t'));
            }
        }
        out
    }

    fn lower_and(&mut self, left: &Stmt, right: &Stmt) -> Result<(), ShellError> {
        self.lower_stmt(left)?;
        let hole = self.chunk.push_placeholder(IrOp::JmpIfFail(0));
        self.lower_stmt(right)?;
        self.chunk.patch(hole, self.chunk.here());
        Ok(())
    }

    fn lower_or(&mut self, left: &Stmt, right: &Stmt) -> Result<(), ShellError> {
        self.lower_stmt(left)?;
        let hole = self.chunk.push_placeholder(IrOp::JmpIfOk(0));
        self.lower_stmt(right)?;
        self.chunk.patch(hole, self.chunk.here());
        Ok(())
    }

    fn lower_background(&mut self, inner: &Stmt) -> Result<(), ShellError> {
        let before = self.chunk.here();
        self.lower_stmt(inner)?;
        let after = self.chunk.here();
        for i in (before..after).rev() {
            if let IrOp::ExecExternal(n) = self.chunk.ops[i] {
                self.chunk.ops[i] = IrOp::ExecExternalBg(n);
                break;
            }
        }
        Ok(())
    }

    fn lower_if(&mut self, stmt: &IfStmt) -> Result<(), ShellError> {
        for s in &stmt.condition {
            self.lower_stmt(s)?;
        }
        let fail_hole = self.chunk.push_placeholder(IrOp::JmpIfFail(0));
        for s in &stmt.then_body {
            self.lower_stmt(s)?;
        }
        if let Some(else_body) = &stmt.else_body {
            let end_hole = self.chunk.push_placeholder(IrOp::Jmp(0));
            let else_start = self.chunk.here();
            self.chunk.patch(fail_hole, else_start);
            for s in else_body {
                self.lower_stmt(s)?;
            }
            self.chunk.patch(end_hole, self.chunk.here());
        } else {
            self.chunk.patch(fail_hole, self.chunk.here());
        }
        Ok(())
    }

    fn lower_while(&mut self, w: &WhileStmt) -> Result<(), ShellError> {
        self.chunk.push(IrOp::RedirSave);
        for redir in &w.redirects {
            if let Some(ir) = self.lower_redirect(redir)? {
                self.chunk.push(IrOp::Redirect(ir));
            }
            for extra in self.extra_redirs.drain(..) {
                self.chunk.push(IrOp::Redirect(extra));
            }
        }
        let loop_start = self.chunk.here();
        for s in &w.condition {
            self.lower_stmt(s)?;
        }
        let exit_jump = if w.negated {
            IrOp::JmpIfOk(0)
        } else {
            IrOp::JmpIfFail(0)
        };
        let exit_hole = self.chunk.push_placeholder(exit_jump);
        self.loop_stack.push(LoopCtx {
            kind: LoopKind::While,
            start: loop_start,
            break_holes: vec![],
        });
        for s in &w.body {
            self.lower_stmt(s)?;
        }
        let ctx = self.loop_stack.pop().unwrap();
        self.chunk.push(IrOp::Jmp(loop_start));
        let loop_end = self.chunk.here();
        self.chunk.patch(exit_hole, loop_end);
        for hole in ctx.break_holes {
            self.chunk.patch(hole, loop_end);
        }
        self.chunk.push(IrOp::StatusOk);
        self.chunk.push(IrOp::RedirRestore);
        Ok(())
    }

    fn lower_for(&mut self, f: &ForStmt) -> Result<(), ShellError> {
        self.chunk.push(IrOp::RedirSave);
        for redir in &f.redirects {
            if let Some(ir) = self.lower_redirect(redir)? {
                self.chunk.push(IrOp::Redirect(ir));
            }
            for extra in self.extra_redirs.drain(..) {
                self.chunk.push(IrOp::Redirect(extra));
            }
        }
        for item in &f.items {
            self.lower_word(item)?;
        }
        self.chunk.push(IrOp::ForSetup(f.items.len()));
        let loop_start = self.chunk.here();
        self.chunk.push(IrOp::ForBind(f.var.clone()));
        let exit_hole = self.chunk.push_placeholder(IrOp::JmpIfFail(0));
        self.loop_stack.push(LoopCtx {
            kind: LoopKind::For,
            start: loop_start,
            break_holes: vec![],
        });
        for s in &f.body {
            self.lower_stmt(s)?;
        }
        let ctx = self.loop_stack.pop().unwrap();
        self.chunk.push(IrOp::Jmp(loop_start));
        let loop_end = self.chunk.here();
        self.chunk.patch(exit_hole, loop_end);
        for hole in ctx.break_holes {
            self.chunk.patch(hole, loop_end);
        }
        self.chunk.push(IrOp::RedirRestore);
        Ok(())
    }

    fn lower_case(&mut self, c: &CaseStmt) -> Result<(), ShellError> {
        // bash never field-splits or globs the case subject — lower it
        // without the GlobExpand pass.
        self.lower_word_single_parts(&c.word.parts)?;
        self.chunk.push(IrOp::CaseBegin);
        let mut end_holes: Vec<usize> = vec![];
        let mut prev_fail_hole: Option<usize> = None;
        for arm in &c.arms {
            if let Some(hole) = prev_fail_hole {
                self.chunk.patch(hole, self.chunk.here());
            }
            let n = arm.patterns.len();
            let mut ok_holes: Vec<usize> = vec![];
            for (i, pat) in arm.patterns.iter().enumerate() {
                if let Some(lit) = pat.as_literal() {
                    self.chunk.push(IrOp::CaseMatch(lit.to_string()));
                } else {
                    self.lower_word_single_parts(&pat.parts)?;
                    self.chunk.push(IrOp::CaseMatchDyn);
                }
                if i < n - 1 {
                    ok_holes.push(self.chunk.push_placeholder(IrOp::JmpIfOk(0)));
                }
            }
            let fail_hole = self.chunk.push_placeholder(IrOp::JmpIfFail(0));
            prev_fail_hole = Some(fail_hole);
            let body_start = self.chunk.here();
            for hole in ok_holes {
                self.chunk.patch(hole, body_start);
            }
            for s in &arm.body {
                self.lower_stmt(s)?;
            }
            end_holes.push(self.chunk.push_placeholder(IrOp::Jmp(0)));
        }
        let case_end = self.chunk.here();
        if let Some(hole) = prev_fail_hole {
            self.chunk.patch(hole, case_end);
        }
        for hole in end_holes {
            self.chunk.patch(hole, case_end);
        }
        self.chunk.push(IrOp::CaseEnd);
        Ok(())
    }

    fn lower_subshell(&mut self, stmts: &[Stmt], redirects: &[Redirect]) -> Result<(), ShellError> {
        for redir in redirects {
            if let Some(ir) = self.lower_redirect(redir)? {
                self.chunk.push(IrOp::Redirect(ir));
            }
            for extra in self.extra_redirs.drain(..) {
                self.chunk.push(IrOp::Redirect(extra));
            }
        }
        self.chunk.push(IrOp::SubshellBegin);
        for s in stmts {
            self.lower_stmt(s)?;
        }
        self.chunk.push(IrOp::SubshellEnd);
        Ok(())
    }

    fn lower_brace_group(
        &mut self,
        stmts: &[Stmt],
        redirects: &[Redirect],
    ) -> Result<(), ShellError> {
        self.chunk.push(IrOp::RedirSave);
        for redir in redirects {
            if let Some(ir) = self.lower_redirect(redir)? {
                self.chunk.push(IrOp::Redirect(ir));
            }
            for extra in self.extra_redirs.drain(..) {
                self.chunk.push(IrOp::Redirect(extra));
            }
        }
        for s in stmts {
            self.lower_stmt(s)?;
        }
        self.chunk.push(IrOp::RedirRestore);
        Ok(())
    }

    // ── Bash [[ ]] conditional lowering ───────────────────────────────────────

    fn lower_bash_cond_stmt(&mut self, stmt: &BashCondStmt) -> Result<(), ShellError> {
        self.lower_bash_cond_expr(&stmt.expr)
    }

    fn lower_bash_cond_expr(&mut self, expr: &BashCondExpr) -> Result<(), ShellError> {
        match expr {
            BashCondExpr::Not(inner) => {
                self.lower_bash_cond_expr(inner)?;
                // Flip status: OK → FAIL, FAIL → OK
                let ok_hole = self.chunk.push_placeholder(IrOp::JmpIfOk(0));
                self.chunk.push(IrOp::StatusOk);
                let end_hole = self.chunk.push_placeholder(IrOp::Jmp(0));
                self.chunk.patch(ok_hole, self.chunk.here());
                self.chunk.push(IrOp::StatusFail);
                self.chunk.patch(end_hole, self.chunk.here());
            }
            BashCondExpr::And(left, right) => {
                self.lower_bash_cond_expr(left)?;
                // Short-circuit: if left is FAIL, skip right
                let fail_hole = self.chunk.push_placeholder(IrOp::JmpIfFail(0));
                self.lower_bash_cond_expr(right)?;
                self.chunk.patch(fail_hole, self.chunk.here());
            }
            BashCondExpr::Or(left, right) => {
                self.lower_bash_cond_expr(left)?;
                // Short-circuit: if left is OK, skip right
                let ok_hole = self.chunk.push_placeholder(IrOp::JmpIfOk(0));
                self.lower_bash_cond_expr(right)?;
                self.chunk.patch(ok_hole, self.chunk.here());
            }
            BashCondExpr::Unary { op, arg } => {
                self.lower_word(arg)?;
                self.chunk.push(IrOp::BashUnary(op.clone()));
            }
            BashCondExpr::Binary { left, op, right } => {
                self.lower_word(left)?;
                self.lower_word(right)?;
                self.chunk.push(IrOp::BashBinary(op.clone()));
            }
        }
        Ok(())
    }

    /// Emit `$(( n++ ))` / `$(( ++n ))` / `$(( n-- ))` / `$(( --n ))` /
    /// `$(( n op= rhs ))` — assignment arithmetic with write-back.
    /// Returns the number of result stack slots (always 1), or None if
    /// `expr` isn't an assignment form.
    fn lower_arith_assign(&mut self, expr: &str) -> Result<Option<usize>, ShellError> {
        let trimmed = expr.trim();
        let bytes = trimmed.as_bytes();

        let ident_len = |s: &[u8], from: usize| -> usize {
            let mut j = from;
            while j < s.len() && (s[j].is_ascii_alphanumeric() || s[j] == b'_') {
                j += 1;
            }
            j - from
        };

        // Postfix: `name++` / `name--`
        let n0 = ident_len(bytes, 0);
        if n0 > 0 {
            let name = &trimmed[..n0];
            let after = &trimmed[n0..];
            if after == "++" || after == "--" {
                let op = &after[..1];
                // Result of the expression is the OLD value.
                let arith_parts = parse_arith_expr_parts(trimmed);
                let n = self.push_parts(&arith_parts)?;
                if n > 1 {
                    self.chunk.push(IrOp::ConcatN(n));
                }
                self.chunk.push(IrOp::ArithEvalStack);
                // Then compute and store the new value.
                self.push_assign_delta(name, op, "1");
                return Ok(Some(1));
            }
        }

        // Prefix: `++name` / `--name`
        if bytes.starts_with(b"++") || bytes.starts_with(b"--") {
            let op = &trimmed[..1];
            let n1 = ident_len(bytes, 2);
            if n1 > 0 && 2 + n1 == trimmed.len() {
                let name = &trimmed[2..2 + n1];
                self.push_assign_delta(name, op, "1");
                // Result of the expression is the NEW value.
                self.chunk.push(IrOp::PushVar(name.to_string()));
                return Ok(Some(1));
            }
        }

        // Compound assign: `name op= rhs` (op in + - * / %)
        if n0 > 0 {
            let name = trimmed[..n0].to_string();
            let rest = &trimmed[n0..];
            for op in ["+", "-", "*", "/", "%"] {
                if let Some(rhs) = rest.strip_prefix(&format!("{op}=")) {
                    let rhs = rhs.trim();
                    if !rhs.is_empty() {
                        // Stack layout (bottom-up): name, " op ", rhs-parts…
                        // ConcatN joins pushed slots in push order → "name op rhs".
                        self.chunk.push(IrOp::PushVar(name.clone()));
                        self.chunk.push(IrOp::PushConst(format!(" {op} ")));
                        let rhs_parts = parse_arith_expr_parts(rhs);
                        let n = self.push_parts(&rhs_parts)?;
                        self.chunk.push(IrOp::ConcatN(n + 2));
                        self.chunk.push(IrOp::ArithEvalStack);
                        self.chunk.push(IrOp::SetVar(name.clone()));
                        // Result of the expression is the new value.
                        self.chunk.push(IrOp::PushVar(name.clone()));
                        return Ok(Some(1));
                    }
                }
            }
        }

        Ok(None)
    }

    /// Helper for ++/--: emit `name op 1` evaluation and `SetVar(name)`.
    /// Leaves the computed value on the stack (consumed by SetVar).
    fn push_assign_delta(&mut self, name: &str, op: &str, delta: &str) {
        self.chunk.push(IrOp::PushVar(name.to_string()));
        self.chunk.push(IrOp::PushConst(format!(" {op} {delta}")));
        self.chunk.push(IrOp::ConcatN(2));
        self.chunk.push(IrOp::ArithEvalStack);
        self.chunk.push(IrOp::SetVar(name.to_string()));
    }
}

impl Default for Lowerer {
    fn default() -> Self {
        Self::new()
    }
}

enum Class {
    Builtin(BuiltinId),
    Break,
    Continue,
    Return,
}

fn classify(name: &str) -> Option<Class> {
    match name {
        "break" => return Some(Class::Break),
        "continue" => return Some(Class::Continue),
        "return" => return Some(Class::Return),
        _ => {}
    }
    BuiltinId::from_name(name).map(Class::Builtin)
}

/// Parse an arithmetic expression template into `WordPart`s so variable
/// references are expanded via `PushVar` at runtime.
///
/// Recognises two forms of variable reference:
///   `$var` / `${var}` — explicit dollar-sign form
///   bare identifiers   — POSIX allows `a + b` (without `$`) inside `$(( ))`
///
/// Numeric tokens (decimals, `0xFF` hex, `077` octal) are kept as literals so
/// letters inside them (like the `x` or `F` in `0xFF`) are not mistaken for
/// variable names.  The rule: a letter/underscore starts a variable name only
/// when the character immediately before it in the accumulating literal is NOT
/// alphanumeric (i.e. it is not the interior of a `0x…` token).
fn parse_arith_expr_parts(expr: &str) -> Vec<shell_ast::ast::WordPart> {
    use shell_ast::ast::WordPart;
    let bytes = expr.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;
    let mut parts = vec![];
    let mut lit = String::new();

    while i < len {
        let b = bytes[i];
        if b == b'$' && i + 1 < len && bytes[i + 1] == b'(' {
            if i + 2 < len && bytes[i + 2] == b'(' {
                let start = i + 3;
                let mut depth = 2usize;
                let mut j = start;
                while j < len && depth > 0 {
                    if bytes[j] == b'(' {
                        depth += 1;
                    } else if bytes[j] == b')' {
                        depth -= 1;
                    }
                    if depth > 0 {
                        j += 1;
                    }
                }
                let inner = expr[start..j].to_string();
                i = if depth == 0 { j + 1 } else { j };
                if !lit.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                }
                parts.push(WordPart::ArithSub(inner));
            } else {
                let start = i + 2;
                let mut depth = 1usize;
                let mut j = start;
                while j < len && depth > 0 {
                    if bytes[j] == b'(' {
                        depth += 1;
                    } else if bytes[j] == b')' {
                        depth -= 1;
                    }
                    if depth > 0 {
                        j += 1;
                    }
                }
                let inner = expr[start..j].to_string();
                i = if depth == 0 { j + 1 } else { j };
                if !lit.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                }
                let tokens = shell_lex::Lexer::new(&inner).tokenize().unwrap_or_default();
                let stmts = shell_parse::Parser::new(tokens)
                    .parse()
                    .map(|s| s.stmts)
                    .unwrap_or_default();
                parts.push(WordPart::CmdSub(stmts));
            }
        } else if b == b'$' && i + 1 < len && bytes[i + 1] == b'{' {
            i += 2;
            let start = i;
            while i < len && bytes[i] != b'}' {
                i += 1;
            }
            let name = expr[start..i].to_string();
            if i < len {
                i += 1;
            }
            if !lit.is_empty() {
                parts.push(WordPart::Literal(std::mem::take(&mut lit)));
            }
            parts.push(WordPart::Var(name));
        } else if b == b'$' {
            i += 1;
            let start = i;
            while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            if i == start {
                lit.push('$');
            } else {
                let name = expr[start..i].to_string();
                if !lit.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                }
                parts.push(WordPart::Var(name));
            }
        } else if b.is_ascii_alphabetic() || b == b'_' {
            let prev_alnum = lit
                .as_bytes()
                .last()
                .map(|&p| p.is_ascii_alphanumeric())
                .unwrap_or(false);
            if prev_alnum {
                lit.push(b as char);
                i += 1;
                while i < len && bytes[i].is_ascii_alphanumeric() {
                    lit.push(bytes[i] as char);
                    i += 1;
                }
            } else {
                if !lit.is_empty() {
                    parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                }
                let start = i;
                i += 1;
                while i < len && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
                let name = expr[start..i].to_string();
                // Array element reference: `arr[i]` — keep it whole so the
                // VM resolves the element instead of substituting the
                // scalar `$arr` and leaving `[i]` as a stray literal.
                if bytes.get(i) == Some(&b'[') {
                    let idx_start = i + 1;
                    let mut j = idx_start;
                    while j < len && bytes[j] != b']' {
                        j += 1;
                    }
                    if j < len {
                        let idx = expr[idx_start..j].to_string();
                        i = j + 1;
                        let op = if idx == "@" || idx == "*" {
                            VarExpandOp::IndexJoin(idx)
                        } else {
                            VarExpandOp::Index(idx)
                        };
                        parts.push(WordPart::VarExpand(VarExpand {
                            var: name,
                            op,
                            modifier: None,
                            span: Default::default(),
                        }));
                        continue;
                    }
                }
                parts.push(WordPart::Var(name));
            }
        } else {
            lit.push(b as char);
            i += 1;
        }
    }

    if !lit.is_empty() {
        parts.push(WordPart::Literal(lit));
    }
    if parts.is_empty() {
        parts.push(WordPart::Literal(String::new()));
    }
    parts
}
