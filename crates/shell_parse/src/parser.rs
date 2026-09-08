use shell_ast::{
    ast::{
        Assignment, BashCondExpr, BashCondStmt, CaseArm, CaseStmt, Command, ForStmt, FunctionDef,
        IfStmt, Pipeline, PipelineArm, PipelineStmt, Redirect, RedirectKind, RedirectTarget,
        Script, Stmt, VarExpand, VarExpandOp, WhileStmt, Word, WordPart,
    },
    ShellError, Span,
};
use shell_lex::token::TokenKind;

use crate::{consume::TokenStream, precedence::Prec};

pub struct Parser {
    stream: TokenStream,
}

impl Parser {
    pub fn new(tokens: Vec<shell_lex::Token>) -> Self {
        Self {
            stream: TokenStream::new(tokens),
        }
    }

    pub fn parse(mut self) -> Result<Script, ShellError> {
        let start = self.stream.current_span();
        let mut stmts = vec![];
        loop {
            self.stream.skip_terminators();
            if self.stream.is_eof() {
                break;
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(Script::new(stmts, start.merge(self.stream.current_span())))
    }

    fn parse_stmt(&mut self) -> Result<Stmt, ShellError> {
        let stmt = self.parse_bool_chain(Prec::None)?;
        if self.stream.eat(&TokenKind::Ampersand).is_some() {
            return Ok(Stmt::Background(Box::new(stmt)));
        }
        match self.stream.peek_kind() {
            TokenKind::Semi | TokenKind::Newline => {
                self.stream.advance();
            }
            _ => {}
        }
        Ok(stmt)
    }

    fn parse_bool_chain(&mut self, min_prec: Prec) -> Result<Stmt, ShellError> {
        let mut left = self.parse_pipeline_stmt()?;
        loop {
            let op_prec = match Prec::of(self.stream.peek_kind()) {
                Some(p) if p > min_prec => p,
                _ => break,
            };
            self.stream.advance();
            self.stream.skip_newlines();
            let right = self.parse_bool_chain(op_prec)?;
            left = match op_prec {
                Prec::And => Stmt::And(Box::new(left), Box::new(right)),
                Prec::Or => Stmt::Or(Box::new(left), Box::new(right)),
                Prec::None => unreachable!(),
            };
        }
        Ok(left)
    }

    fn parse_pipeline_stmt(&mut self) -> Result<Stmt, ShellError> {
        // `! pipeline` — negate exit status (POSIX). Must be checked before
        // command parsing so `!` is not treated as a command name.
        if let TokenKind::Word(s) = self.stream.peek_kind() {
            if s == "!" {
                self.stream.advance();
                let inner = self.parse_pipeline_stmt()?;
                return Ok(Stmt::Negate(Box::new(inner)));
            }
        }

        // Bash [[ ... ]] conditional
        if let TokenKind::Word(s) = self.stream.peek_kind() {
            if s == "[[" {
                return self.parse_bash_cond();
            }
        }

        // Compound statements that are NOT pipeline first-arms go here.
        // However, if they are followed by `|`, they become a pipeline arm.
        match self.stream.peek_kind() {
            TokenKind::If => return self.parse_if(),
            TokenKind::While | TokenKind::Until => return self.parse_while(),
            TokenKind::For => return self.parse_for(),
            TokenKind::Case => return self.parse_case(),
            TokenKind::Function => return self.parse_function_keyword(),
            TokenKind::LBrace => {
                // Could be a lone brace group OR the first arm of a pipeline.
                let stmt = self.parse_brace_group()?;
                if !matches!(self.stream.peek_kind(), TokenKind::Pipe) {
                    return Ok(stmt);
                }
                // Fall through to build a pipeline with this as the first arm.
                let (stmts, redirs) = match stmt {
                    Stmt::BraceGroup {
                        stmts: s,
                        redirects: r,
                    } => (s, r),
                    s => return Ok(s),
                };
                return self.parse_pipeline_from_first_arm(
                    PipelineArm::BraceGroup(stmts, redirs),
                    vec![],
                    vec![],
                );
            }
            TokenKind::LParen => {
                // Could be a lone subshell OR the first arm of a pipeline.
                let stmt = self.parse_subshell()?;
                if !matches!(self.stream.peek_kind(), TokenKind::Pipe) {
                    return Ok(stmt);
                }
                let (stmts, redirs) = match stmt {
                    Stmt::Subshell {
                        stmts: s,
                        redirects: r,
                    } => (s, r),
                    s => return Ok(s),
                };
                return self.parse_pipeline_from_first_arm(
                    PipelineArm::Subshell(stmts, redirs),
                    vec![],
                    vec![],
                );
            }
            _ => {}
        }

        if self.is_function_def_ahead() {
            return self.parse_function_posix();
        }

        let start = self.stream.current_span();
        let (first_cmd, first_pending) = self.parse_command()?;
        let pending_heredocs: Vec<(usize, usize)> =
            first_pending.into_iter().map(|idx| (0, idx)).collect();
        let first_arm = PipelineArm::Simple(first_cmd);

        if !matches!(self.stream.peek_kind(), TokenKind::Pipe) {
            // Single-command pipeline — still go through the common path
            // so heredocs are attached correctly.
            let mut cmds_for_heredoc = match &first_arm {
                PipelineArm::Simple(c) => vec![c.clone()],
                _ => vec![],
            };
            let mut ph = pending_heredocs.clone();
            self.attach_pending_heredoc_bodies(&mut cmds_for_heredoc, &mut ph)?;
            let span = start.merge(self.stream.current_span());
            let first_arm2 = PipelineArm::Simple(cmds_for_heredoc.into_iter().next().unwrap());
            return Ok(Stmt::Command(PipelineStmt {
                pipeline: Pipeline {
                    arms: vec![first_arm2],
                    span,
                },
                span,
            }));
        }

        self.parse_pipeline_from_first_arm(first_arm, pending_heredocs, vec![start])
    }

    /// Continue parsing `| stage | stage …` after the first arm has been parsed.
    fn parse_pipeline_from_first_arm(
        &mut self,
        first_arm: PipelineArm,
        mut first_pending: Vec<(usize, usize)>,
        starts: Vec<Span>,
    ) -> Result<Stmt, ShellError> {
        let start = starts
            .into_iter()
            .next()
            .unwrap_or_else(|| self.stream.current_span());

        // Collect Simple-command-only pending heredocs (arms 0 = first_arm).
        let mut simple_cmds: Vec<Command> = match &first_arm {
            PipelineArm::Simple(c) => vec![c.clone()],
            _ => vec![],
        };
        let mut arms: Vec<PipelineArm> = vec![first_arm];

        while self.stream.eat(&TokenKind::Pipe).is_some() {
            self.stream.skip_newlines();
            let arm = match self.stream.peek_kind() {
                TokenKind::LParen => {
                    let stmt = self.parse_subshell()?;
                    match stmt {
                        Stmt::Subshell {
                            stmts: s,
                            redirects: r,
                        } => PipelineArm::Subshell(s, r),
                        s => {
                            return Err(ShellError::parse(
                                self.stream.current_span(),
                                &format!("unexpected statement in pipeline: {:?}", s),
                            ))
                        }
                    }
                }
                TokenKind::LBrace => {
                    let stmt = self.parse_brace_group()?;
                    match stmt {
                        Stmt::BraceGroup {
                            stmts: s,
                            redirects: r,
                        } => PipelineArm::BraceGroup(s, r),
                        s => {
                            return Err(ShellError::parse(
                                self.stream.current_span(),
                                &format!("unexpected statement in pipeline: {:?}", s),
                            ))
                        }
                    }
                }
                // Compound commands (while/for/if/case) as pipeline stages.
                // Bash semantics: they run in a subshell when used mid-pipeline.
                TokenKind::While | TokenKind::For | TokenKind::If | TokenKind::Case => {
                    let stmt = self.parse_pipeline_stmt()?;
                    PipelineArm::Subshell(vec![stmt], vec![])
                }
                _ => {
                    let base = simple_cmds.len();
                    let (cmd, local_pending) = self.parse_command()?;
                    for idx in local_pending {
                        first_pending.push((base, idx));
                    }
                    simple_cmds.push(cmd.clone());
                    PipelineArm::Simple(cmd)
                }
            };
            arms.push(arm);
        }

        // Attach heredoc bodies only to Simple arms.
        self.attach_pending_heredoc_bodies(&mut simple_cmds, &mut first_pending)?;
        // Write back patched simple cmds into arms.
        let mut simple_iter = simple_cmds.into_iter();
        for arm in &mut arms {
            if let PipelineArm::Simple(c) = arm {
                if let Some(patched) = simple_iter.next() {
                    *c = patched;
                }
            }
        }

        let span = start.merge(self.stream.current_span());
        Ok(Stmt::Command(PipelineStmt {
            pipeline: Pipeline { arms, span },
            span,
        }))
    }

    // ── Bash [[ ]] conditional ─────────────────────────────────────────────────

    fn parse_bash_cond(&mut self) -> Result<Stmt, ShellError> {
        let start = self.stream.advance_span(); // consume [[
        self.stream.skip_newlines();
        let expr = self.parse_bash_cond_or()?;
        self.stream.skip_newlines();
        // expect ]]
        match self.stream.peek_kind().clone() {
            TokenKind::Word(s) if s == "]]" => {
                self.stream.advance();
            }
            other => {
                return Err(ShellError::unexpected_token(
                    other.to_string(),
                    self.stream.current_span(),
                ))
            }
        }
        let span = start.merge(self.stream.current_span());
        Ok(Stmt::BashCond(BashCondStmt { expr, span }))
    }

    fn parse_bash_cond_or(&mut self) -> Result<BashCondExpr, ShellError> {
        let mut left = self.parse_bash_cond_and()?;
        loop {
            if !matches!(self.stream.peek_kind(), TokenKind::Or) {
                break;
            }
            self.stream.advance();
            self.stream.skip_newlines();
            let right = self.parse_bash_cond_and()?;
            left = BashCondExpr::Or(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_bash_cond_and(&mut self) -> Result<BashCondExpr, ShellError> {
        let mut left = self.parse_bash_cond_not()?;
        loop {
            if !matches!(self.stream.peek_kind(), TokenKind::And) {
                break;
            }
            self.stream.advance();
            self.stream.skip_newlines();
            let right = self.parse_bash_cond_not()?;
            left = BashCondExpr::And(Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn parse_bash_cond_not(&mut self) -> Result<BashCondExpr, ShellError> {
        if let TokenKind::Word(s) = self.stream.peek_kind().clone() {
            if s == "!" {
                self.stream.advance();
                let inner = self.parse_bash_cond_not()?;
                return Ok(BashCondExpr::Not(Box::new(inner)));
            }
        }
        self.parse_bash_cond_primary()
    }

    fn parse_bash_cond_primary(&mut self) -> Result<BashCondExpr, ShellError> {
        // Grouping: ( expr )
        if matches!(self.stream.peek_kind(), TokenKind::LParen) {
            self.stream.advance();
            let expr = self.parse_bash_cond_or()?;
            self.stream.expect(&TokenKind::RParen)?;
            return Ok(expr);
        }

        // Peek at the first word-ish token
        let first_lit = match self.stream.peek_kind() {
            TokenKind::Word(s) => Some(s.clone()),
            _ => None,
        };

        // Unary file/string tests: -f, -d, -e, -z, -n, -r, -w, -x, -s, -L, -h …
        if let Some(ref op) = first_lit {
            if is_bash_unary_op(op) {
                let op = op.clone();
                self.stream.advance();
                let arg = self.parse_bash_word()?;
                return Ok(BashCondExpr::Unary { op, arg });
            }
        }

        // Must be a word (left side of binary, or bare non-empty test)
        let left = self.parse_bash_word()?;

        // Peek operator
        let op_opt = self.peek_bash_binary_op();
        let op = match op_opt {
            Some(op) => op,
            None => {
                // No binary op → bare word treated as non-empty string test
                return Ok(BashCondExpr::Unary {
                    op: "-n".to_string(),
                    arg: left,
                });
            }
        };
        self.consume_bash_binary_op(&op);
        let right = self.parse_bash_word()?;
        Ok(BashCondExpr::Binary { left, op, right })
    }

    /// Peek at the current token to see if it's a bash binary operator.
    /// Returns None if the next token signals end of primary (]], &&, ||, )).
    fn peek_bash_binary_op(&self) -> Option<String> {
        match self.stream.peek_kind() {
            // terminators / logical ops → not a binary op
            TokenKind::And | TokenKind::Or | TokenKind::RParen => return None,
            TokenKind::Word(s) if s == "]]" || s == "!" => return None,
            TokenKind::Word(s) if is_bash_binary_op(s) => return Some(s.clone()),
            // < and > inside [[ ]] are string comparison, not redirects
            TokenKind::RedirIn => return Some("<".to_string()),
            TokenKind::RedirOut => return Some(">".to_string()),
            // something unknown at end → bail
            _ => None,
        }
    }

    fn consume_bash_binary_op(&mut self, op: &str) {
        match op {
            "<" | ">" => {
                self.stream.advance();
            } // consume redirect token
            _ => {
                self.stream.advance();
            } // consume Word token
        }
    }

    /// Like parse_word but inside [[ ... ]] where `{` and `}` are NOT
    /// metacharacters (they appear literally in ERE quantifiers like `{3,5}`).
    /// Stitches together adjacent Word / LBrace / RBrace tokens into one Word.
    ///
    /// Example: `^[a-z]{3}$` is lexed as Word("^[a-z]") LBrace Word("3") RBrace
    /// Word("$") — five tokens.  This function joins them into a single Word so
    /// the ERE engine sees the full pattern `^[a-z]{3}$`.
    fn parse_bash_word(&mut self) -> Result<Word, ShellError> {
        let start = self.stream.current_span();
        let first = self.parse_word()?;
        let mut all_parts = first.parts;

        loop {
            match self.stream.peek_kind() {
                TokenKind::LBrace => {
                    // ERE {n} / {n,m} quantifier — stitch into current word
                    self.stream.advance();
                    all_parts.push(WordPart::Literal("{".to_string()));
                    loop {
                        match self.stream.peek_kind().clone() {
                            TokenKind::RBrace => {
                                self.stream.advance();
                                all_parts.push(WordPart::Literal("}".to_string()));
                                break;
                            }
                            TokenKind::Word(_)
                            | TokenKind::SingleQuoted(_)
                            | TokenKind::DoubleQuoted(_) => {
                                let w = self.parse_word()?;
                                all_parts.extend(w.parts);
                            }
                            _ => break, // malformed — bail without consuming
                        }
                    }
                }
                // Also stitch a following bare Word token when it is not a
                // bash-conditional terminator or operator.  This covers the
                // trailing `$` in `^[a-z]{3}$` which the lexer emits as a
                // separate Word("$") token after the RBrace.
                TokenKind::Word(_) => {
                    // Clone to avoid holding a borrow while we call parse_word
                    // (which needs &mut self).
                    let s = match self.stream.peek_kind() {
                        TokenKind::Word(s) => s.clone(),
                        _ => break,
                    };
                    if bash_word_is_terminator(&s) {
                        break;
                    }
                    let w = self.parse_word()?;
                    all_parts.extend(w.parts);
                }
                _ => break,
            }
        }

        Ok(Word {
            parts: all_parts,
            span: start.merge(self.stream.current_span()),
            may_glob: false,
        })
    }

    // ── Helper predicates ──────────────────────────────────────────────────────

    fn is_function_def_ahead(&self) -> bool {
        matches!(self.stream.peek_kind(), TokenKind::Word(_))
            && matches!(self.stream.peek_at(1).kind, TokenKind::LParen)
            && matches!(self.stream.peek_at(2).kind, TokenKind::RParen)
            // `name=` / `name+=` before `( )` is an array literal, not a
            // function definition (`arr=(x y)` / `e=()`).
            && !matches!(
                self.stream.peek_kind(),
                TokenKind::Word(ref w) if w.contains('=') || w.contains('+')
            )
    }

    fn attach_pending_heredoc_bodies(
        &mut self,
        cmds: &mut [Command],
        pending_heredocs: &mut Vec<(usize, usize)>,
    ) -> Result<(), ShellError> {
        while let TokenKind::HereDocBody(body) = self.stream.peek_kind().clone() {
            self.stream.advance();
            let Some((cmd_idx, redir_idx)) = pending_heredocs.first().copied() else {
                return Err(ShellError::parse(
                    self.stream.current_span(),
                    "unexpected heredoc body",
                ));
            };
            let Some(cmd) = cmds.get_mut(cmd_idx) else {
                return Err(ShellError::parse(
                    self.stream.current_span(),
                    "internal heredoc command index error",
                ));
            };
            let Some(redir) = cmd.redirects.get_mut(redir_idx) else {
                return Err(ShellError::parse(
                    self.stream.current_span(),
                    "internal heredoc redirect index error",
                ));
            };
            redir.target = RedirectTarget::HereDoc(body);
            pending_heredocs.remove(0);
        }
        Ok(())
    }

    /// Parse `arr=( w1 w2 … )` starting at the `(`. Consumes the parens;
    /// returns the array assignment with the elements' flattened parts.
    fn parse_array_literal(
        &mut self,
        mut assign: Assignment,
        span: Span,
    ) -> Result<Assignment, ShellError> {
        self.stream.expect(&TokenKind::LParen)?;
        let mut elems: Vec<Word> = vec![];
        loop {
            match self.stream.peek_kind() {
                TokenKind::RParen => {
                    self.stream.advance();
                    break;
                }
                TokenKind::Eof => {
                    return Err(ShellError::parse(
                        self.stream.current_span(),
                        "unterminated array literal — missing ')'",
                    ))
                }
                TokenKind::Word(_) | TokenKind::SingleQuoted(_) | TokenKind::DoubleQuoted(_) => {
                    let mut word = self.parse_word()?;
                    while let Some(prev_end) = self.stream.prev_token().map(|t| t.span.end) {
                        if !matches!(
                            self.stream.peek_kind(),
                            TokenKind::Word(_)
                                | TokenKind::SingleQuoted(_)
                                | TokenKind::DoubleQuoted(_)
                        ) {
                            break;
                        }
                        if self.stream.peek().span.start != prev_end {
                            break;
                        }
                        let next = self.parse_word()?;
                        word.span = word.span.merge(next.span);
                        word.may_glob |= next.may_glob;
                        word.parts.extend(next.parts);
                    }
                    elems.push(word);
                }
                TokenKind::LBrace => match self.try_brace_expand_in_elems(&mut elems)? {
                    Some(expanded) => {
                        for w in expanded {
                            elems.push(w);
                        }
                    }
                    None => {
                        let lit = self.consume_brace_as_literal();
                        let span = self.stream.current_span();
                        elems.push(Word::literal(lit, span));
                    }
                },
                TokenKind::Semi | TokenKind::Newline => {
                    return Err(ShellError::parse(
                        self.stream.current_span(),
                        "newline/';' inside array literal — each element is a word",
                    ))
                }
                _ => {
                    return Err(ShellError::parse(
                        self.stream.current_span(),
                        "unsupported token in array literal",
                    ))
                }
            }
        }
        assign.elements = elems;
        assign.is_array = true;
        assign.span = span;
        Ok(assign)
    }

    fn parse_command(&mut self) -> Result<(Command, Vec<usize>), ShellError> {
        let start = self.stream.current_span();
        let mut assigns: Vec<Assignment> = vec![];
        let mut local_arrays: Vec<Assignment> = vec![];
        let mut argv: Vec<Word> = vec![];
        let mut redirects: Vec<Redirect> = vec![];
        let mut heredoc_redirects: Vec<usize> = vec![];

        loop {
            if self.is_redirect_start() {
                let redir = self.parse_redirect()?;
                if matches!(
                    redir.kind,
                    RedirectKind::HereDoc | RedirectKind::HereDocStrip
                ) {
                    heredoc_redirects.push(redirects.len());
                }
                redirects.push(redir);
                continue;
            }
            match self.stream.peek_kind() {
                TokenKind::Word(_) | TokenKind::SingleQuoted(_) | TokenKind::DoubleQuoted(_) => {
                    let mut word = self.parse_word()?;
                    // Adjacent quoted/unquoted fragments belong to one word:
                    // `ab"cd"ef` or `"x"'y'` (no whitespace between tokens).
                    while let Some(prev_end) = self.stream.prev_token().map(|t| t.span.end) {
                        if !matches!(
                            self.stream.peek_kind(),
                            TokenKind::Word(_)
                                | TokenKind::SingleQuoted(_)
                                | TokenKind::DoubleQuoted(_)
                        ) {
                            break;
                        }
                        if self.stream.peek().span.start != prev_end {
                            break;
                        }
                        let next = self.parse_word()?;
                        word.span = word.span.merge(next.span);
                        word.may_glob |= next.may_glob;
                        word.parts.extend(next.parts);
                    }
                    if argv.is_empty() {
                        if let Some(a) = try_parse_assignment(&word) {
                            // `arr=(` — the assignment word ends empty and the
                            // next token opens the array element list.
                            let empty = a.value.is_empty() && a.index.is_none();
                            if empty && matches!(self.stream.peek_kind(), TokenKind::LParen) {
                                let a = self.parse_array_literal(a, word.span)?;
                                assigns.push(a);
                                continue;
                            }
                            assigns.push(a);
                            continue;
                        }
                    }
                    // `local name=(…)` — argv[0] is a declaration builtin and
                    // this word is `name=`/`name+=` followed by `(`.
                    if local_decl_cmd(argv.as_slice())
                        && matches!(self.stream.peek_kind(), TokenKind::LParen)
                    {
                        if let Some(mut a) = try_parse_assignment(&word) {
                            let empty = a.value.is_empty() && a.index.is_none();
                            if empty {
                                a.local_decl = true;
                                let a = self.parse_array_literal(a, word.span)?;
                                local_arrays.push(a);
                                continue;
                            }
                        }
                    }
                    argv.push(word);
                }
                TokenKind::LBrace => match self.try_brace_expand_in_command(&mut argv)? {
                    Some(expanded) => {
                        for w in expanded {
                            argv.push(w);
                        }
                    }
                    None => {
                        let lit = self.consume_brace_as_literal();
                        let span = self.stream.current_span();
                        argv.push(Word::literal(lit, span));
                    }
                },
                ref kind if !argv.is_empty() && kind.as_keyword_str().is_some() => {
                    let word = self.parse_word()?;
                    argv.push(word);
                }
                _ => break,
            }
        }

        if assigns.is_empty() && argv.is_empty() && local_arrays.is_empty() && redirects.is_empty()
        {
            let tok = self.stream.peek();
            return Err(ShellError::unexpected_token(tok.kind.to_string(), tok.span));
        }

        let span = start.merge(self.stream.current_span());
        let mut cmd = Command::new(assigns, argv, redirects, span);
        cmd.local_arrays = local_arrays;
        Ok((cmd, heredoc_redirects))
    }

    fn is_redirect_start(&self) -> bool {
        match self.stream.peek_kind() {
            TokenKind::RedirOut
            | TokenKind::RedirAppend
            | TokenKind::RedirIn
            | TokenKind::RedirOutFd
            | TokenKind::RedirInFd
            | TokenKind::HereDoc
            | TokenKind::HereDocStrip
            | TokenKind::HereString => true,
            TokenKind::Word(s) => {
                s.chars().all(|c| c.is_ascii_digit())
                    && matches!(
                        self.stream.peek_at(1).kind,
                        TokenKind::RedirOut
                            | TokenKind::RedirAppend
                            | TokenKind::RedirIn
                            | TokenKind::RedirOutFd
                            | TokenKind::RedirInFd
                            | TokenKind::HereDoc
                            | TokenKind::HereDocStrip
                            | TokenKind::HereString
                    )
            }
            _ => false,
        }
    }

    fn parse_redirect(&mut self) -> Result<Redirect, ShellError> {
        let start = self.stream.current_span();
        let explicit_fd: Option<u32> = match self.stream.peek_kind() {
            TokenKind::Word(s) if s.chars().all(|c| c.is_ascii_digit()) => {
                let fd: u32 = s
                    .parse()
                    .map_err(|_| ShellError::parse(start, "invalid fd"))?;
                self.stream.advance();
                Some(fd)
            }
            _ => None,
        };

        let kind = match self.stream.peek_kind() {
            TokenKind::RedirOut => RedirectKind::Out,
            TokenKind::RedirAppend => RedirectKind::Append,
            TokenKind::RedirIn => RedirectKind::In,
            TokenKind::RedirOutFd => RedirectKind::OutFd,
            TokenKind::RedirInFd => RedirectKind::InFd,
            TokenKind::HereDoc => RedirectKind::HereDoc,
            TokenKind::HereDocStrip => RedirectKind::HereDocStrip,
            TokenKind::HereString => RedirectKind::HereString,
            other => {
                return Err(ShellError::unexpected_token(
                    other.to_string(),
                    self.stream.current_span(),
                ))
            }
        };
        self.stream.advance();

        if matches!(kind, RedirectKind::HereDoc | RedirectKind::HereDocStrip) {
            // A quoted delimiter (any quoted fragment) disables expansion in
            // the heredoc body: <<'EOF' <<\"EOF\" <<E'OF'
            let delim_quoted = matches!(
                self.stream.peek_kind(),
                TokenKind::SingleQuoted(_) | TokenKind::DoubleQuoted(_)
            );
            let _delimiter = self.parse_word()?;
            return Ok(Redirect {
                kind,
                fd: explicit_fd,
                target: RedirectTarget::HereDoc(String::new()),
                no_expand: delim_quoted,
                span: start.merge(self.stream.current_span()),
            });
        }

        if matches!(kind, RedirectKind::OutFd | RedirectKind::InFd) {
            let target = match self.stream.peek_kind() {
                TokenKind::Word(s) if s.chars().all(|c| c.is_ascii_digit()) => {
                    let n: u32 = s
                        .parse()
                        .map_err(|_| ShellError::parse(self.stream.current_span(), "invalid fd"))?;
                    self.stream.advance();
                    RedirectTarget::Fd(n)
                }
                _ => RedirectTarget::File(self.parse_word()?),
            };
            return Ok(Redirect {
                kind,
                fd: explicit_fd,
                target,
                no_expand: false,
                span: start.merge(self.stream.current_span()),
            });
        }

        let word = self.parse_word()?;
        let span = start.merge(self.stream.current_span());
        Ok(Redirect {
            kind,
            fd: explicit_fd,
            target: RedirectTarget::File(word),
            no_expand: false,
            span,
        })
    }

    fn parse_word(&mut self) -> Result<Word, ShellError> {
        let start = self.stream.current_span();
        match self.stream.peek_kind().clone() {
            TokenKind::Word(s) => {
                self.stream.advance();
                let (parts, may_glob) = self.expand_parts(&s, start)?;
                Ok(Word {
                    parts,
                    span: start.merge(self.stream.current_span()),
                    may_glob,
                })
            }
            TokenKind::SingleQuoted(s) => {
                self.stream.advance();
                Ok(Word {
                    parts: vec![WordPart::Literal(s)],
                    span: start.merge(self.stream.current_span()),
                    may_glob: false,
                })
            }
            TokenKind::DoubleQuoted(s) => {
                self.stream.advance();
                let parts = self.expand_dquote_parts(&s, start)?;
                Ok(Word {
                    parts,
                    span: start.merge(self.stream.current_span()),
                    may_glob: false,
                })
            }
            ref kind if kind.as_keyword_str().is_some() => {
                let kw = kind.as_keyword_str().unwrap();
                self.stream.advance();
                Ok(Word {
                    parts: vec![WordPart::Literal(kw.to_string())],
                    span: start.merge(self.stream.current_span()),
                    may_glob: false,
                })
            }
            other => Err(ShellError::unexpected_token(
                other.to_string(),
                self.stream.current_span(),
            )),
        }
    }

    // ── Word expansion ─────────────────────────────────────────────────────────

    fn expand_parts(&self, s: &str, span: Span) -> Result<(Vec<WordPart>, bool), ShellError> {
        let mut parts = vec![];
        let mut chars = s.char_indices().peekable();
        let mut lit = String::new();
        let mut may_glob = false; // true when unquoted * ? [ found

        while let Some((_, c)) = chars.next() {
            match c {
                '"' => {
                    let section = self.expand_dquote_inner(&mut chars, span)?;
                    for p in section {
                        match p {
                            WordPart::Literal(s2) => lit.push_str(&s2),
                            other => {
                                if !lit.is_empty() {
                                    parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                                }
                                parts.push(other);
                            }
                        }
                    }
                }
                '\'' => {
                    let mut inner = String::new();
                    loop {
                        match chars.next() {
                            None | Some((_, '\'')) => break,
                            Some((_, c)) => inner.push(c),
                        }
                    }
                    lit.push_str(&inner);
                }
                '`' => {
                    if !lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                    }
                    let mut inner = String::new();
                    loop {
                        match chars.next() {
                            None | Some((_, '`')) => break,
                            Some((_, c)) => inner.push(c),
                        }
                    }
                    parts.push(WordPart::CmdSub(self.parse_inner(&inner, span)?));
                }
                '$' => {
                    match chars.peek().map(|&(_, c)| c) {
                        Some('(') => {
                            if !lit.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                            }
                            chars.next(); // consume first (
                            if chars.peek().map(|&(_, c)| c) == Some('(') {
                                // Arithmetic substitution: $(( expr ))
                                chars.next(); // consume second (
                                let inner = read_arith_body(&mut chars);
                                parts.push(WordPart::ArithSub(inner));
                            } else {
                                // Command substitution: $( cmd )
                                let inner = read_cmdsub_body(&mut chars);
                                parts.push(WordPart::CmdSub(self.parse_inner(&inner, span)?));
                            }
                        }
                        Some('{') => {
                            chars.next();
                            if !lit.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                            }
                            parts.push(self.parse_brace_expand(&mut chars, span)?);
                            // Unquoted ${...} is subject to field splitting
                            // (and pathname expansion) like $var.
                            may_glob = true;
                        }
                        Some(c2) if c2.is_alphanumeric() || c2 == '_' => {
                            if !lit.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                            }
                            let mut var = String::new();
                            while let Some(&(_, c3)) = chars.peek() {
                                if c3.is_alphanumeric() || c3 == '_' {
                                    var.push(c3);
                                    chars.next();
                                } else {
                                    break;
                                }
                            }
                            parts.push(WordPart::Var(var));
                            may_glob = true;
                        }
                        Some(c2) if is_special_var(c2) => {
                            if !lit.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                            }
                            chars.next();
                            parts.push(WordPart::Var(c2.to_string()));
                            may_glob = true;
                        }
                        _ => lit.push('$'),
                    }
                }
                '*' | '?' => {
                    lit.push(c);
                    may_glob = true;
                }
                '[' => {
                    lit.push(c);
                    may_glob = true;
                }
                c => lit.push(c),
            }
        }

        if !lit.is_empty() {
            parts.push(WordPart::Literal(lit));
        }
        if parts.is_empty() {
            parts.push(WordPart::Literal(String::new()));
        }
        Ok((parts, may_glob))
    }

    fn expand_dquote_parts(&self, s: &str, span: Span) -> Result<Vec<WordPart>, ShellError> {
        let mut chars = s.char_indices().peekable();
        self.expand_dquote_inner(&mut chars, span)
    }

    fn expand_dquote_inner(
        &self,
        chars: &mut std::iter::Peekable<std::str::CharIndices>,
        span: Span,
    ) -> Result<Vec<WordPart>, ShellError> {
        let mut parts = vec![];
        let mut lit = String::new();

        loop {
            match chars.peek().map(|&(_, c)| c) {
                None | Some('"') => {
                    chars.next();
                    break;
                }
                _ => {}
            }
            let (_, c) = chars.next().unwrap();
            match c {
                '\\' => match chars.next().map(|(_, c)| c) {
                    Some('"') => lit.push('"'),
                    Some('\\') => lit.push('\\'),
                    Some('$') => lit.push('$'),
                    Some('`') => lit.push('`'),
                    Some('\n') => {}
                    Some(c2) => {
                        lit.push('\\');
                        lit.push(c2);
                    }
                    None => lit.push('\\'),
                },
                '`' => {
                    if !lit.is_empty() {
                        parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                    }
                    let mut inner = String::new();
                    loop {
                        match chars.next() {
                            None | Some((_, '`')) => break,
                            Some((_, c)) => inner.push(c),
                        }
                    }
                    parts.push(WordPart::CmdSub(self.parse_inner(&inner, span)?));
                }
                '$' => {
                    match chars.peek().map(|&(_, c)| c) {
                        Some('(') => {
                            if !lit.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                            }
                            chars.next(); // consume first (
                            if chars.peek().map(|&(_, c)| c) == Some('(') {
                                // Arithmetic: $(( expr ))
                                chars.next(); // consume second (
                                let inner = read_arith_body(chars);
                                parts.push(WordPart::ArithSub(inner));
                            } else {
                                // Command sub: $( cmd )
                                let inner = read_cmdsub_body(chars);
                                parts.push(WordPart::CmdSub(self.parse_inner(&inner, span)?));
                            }
                        }
                        Some('{') => {
                            chars.next();
                            if !lit.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                            }
                            parts.push(self.parse_brace_expand(chars, span)?);
                        }
                        Some(c2) if c2.is_alphanumeric() || c2 == '_' => {
                            if !lit.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                            }
                            let mut var = String::new();
                            while let Some(&(_, c3)) = chars.peek() {
                                if c3.is_alphanumeric() || c3 == '_' {
                                    var.push(c3);
                                    chars.next();
                                } else {
                                    break;
                                }
                            }
                            parts.push(WordPart::Var(var));
                        }
                        Some(c2) if is_special_var(c2) => {
                            if !lit.is_empty() {
                                parts.push(WordPart::Literal(std::mem::take(&mut lit)));
                            }
                            chars.next();
                            parts.push(WordPart::Var(c2.to_string()));
                        }
                        _ => lit.push('$'),
                    }
                }
                c => lit.push(c),
            }
        }

        if !lit.is_empty() {
            parts.push(WordPart::Literal(lit));
        }
        Ok(parts)
    }

    fn parse_inner(&self, src: &str, span: Span) -> Result<Vec<shell_ast::ast::Stmt>, ShellError> {
        use shell_lex::Lexer;
        let tokens = Lexer::new(src)
            .tokenize()
            .map_err(|e| ShellError::parse(span, &format!("in command substitution: {}", e)))?;
        let script = Parser {
            stream: crate::consume::TokenStream::new(tokens),
        }
        .parse()?;
        Ok(script.stmts)
    }

    fn parse_brace_expand(
        &self,
        chars: &mut std::iter::Peekable<std::str::CharIndices>,
        span: Span,
    ) -> Result<WordPart, ShellError> {
        // Handle ${#…}: either ${#} == $# or ${#varname} == length of varname.
        if chars.peek().map(|&(_, c)| c) == Some('#') {
            chars.next(); // consume '#'
            let next = chars.peek().map(|&(_, c)| c);
            if next == Some('}') {
                chars.next(); // ${#} => $#
                return Ok(WordPart::Var("#".to_string()));
            } else if next
                .map(|c| c.is_alphanumeric() || c == '_')
                .unwrap_or(false)
            {
                // ${#varname} — string length
                let mut var = String::new();
                while let Some(&(_, c)) = chars.peek() {
                    if c.is_alphanumeric() || c == '_' {
                        var.push(c);
                        chars.next();
                    } else {
                        break;
                    }
                }
                // `${#arr[@]}` / `${#arr[*]}` — element count.
                if chars.peek().map(|&(_, c)| c) == Some('[') {
                    chars.next();
                    let mut idx = String::new();
                    while let Some(&(_, c)) = chars.peek() {
                        if c == ']' {
                            break;
                        }
                        idx.push(c);
                        chars.next();
                    }
                    match chars.next() {
                        Some((_, ']')) => {}
                        _ => return Err(ShellError::parse(span, "expected ']' in array index")),
                    }
                    match chars.next() {
                        Some((_, '}')) => {}
                        _ => return Err(ShellError::parse(span, "expected '}' after array index")),
                    }
                    return Ok(WordPart::VarExpand(VarExpand {
                        var,
                        op: VarExpandOp::ArrayLength(idx),
                        modifier: None,
                        span,
                    }));
                }
                match chars.next() {
                    Some((_, '}')) => {}
                    _ => return Err(ShellError::parse(span, "expected '}' after ${#varname}")),
                }
                return Ok(WordPart::VarExpand(VarExpand {
                    var,
                    op: VarExpandOp::Length,
                    modifier: None,
                    span,
                }));
            } else {
                return Err(ShellError::parse(
                    span,
                    "expected variable name or '}' after ${#",
                ));
            }
        }

        // Other special single-char variables: $*, $@, $?, $!, $$, $-, $0
        if let Some(&(_, c)) = chars.peek() {
            if is_special_var(c) {
                chars.next();
                match chars.next() {
                    Some((_, '}')) => {}
                    _ => return Err(ShellError::parse(span, "expected '}'")),
                }
                return Ok(WordPart::Var(c.to_string()));
            }
        }

        // Regular variable name
        let mut var = String::new();
        while let Some(&(_, c)) = chars.peek() {
            if c.is_alphanumeric() || c == '_' {
                var.push(c);
                chars.next();
            } else {
                break;
            }
        }
        if var.is_empty() {
            return Err(ShellError::parse(span, "empty variable name in ${}"));
        }

        // Array index: `${arr[i]}` / `${arr[@]}` — read raw index up to `]`.
        // `]}` closes plain; `]:-`/`]-` continues into a default modifier.
        if chars.peek().map(|&(_, c)| c) == Some('[') {
            chars.next();
            let mut idx = String::new();
            while let Some(&(_, c)) = chars.peek() {
                if c == ']' {
                    break;
                }
                idx.push(c);
                chars.next();
            }
            match chars.next() {
                Some((_, ']')) => {}
                _ => return Err(ShellError::parse(span, "expected ']' in array index")),
            }
            match chars.peek().map(|&(_, c)| c) {
                Some('}') => {
                    chars.next();
                    return Ok(WordPart::VarExpand(VarExpand {
                        var,
                        op: VarExpandOp::Index(idx),
                        modifier: None,
                        span,
                    }));
                }
                // `:-` or bare `-` starts a default modifier.
                Some(':')
                    if {
                        let mut probe = chars.clone();
                        probe.next();
                        probe.next().map(|(_, c)| c) == Some('-')
                    } =>
                {
                    chars.next();
                    chars.next();
                }
                Some('-') => {
                    chars.next();
                }
                _ => return Err(ShellError::parse(span, "expected '}' after array index")),
            }
            // `${arr[i]:-word}` / `${arr[i]-word}` — default modifier.
            let mod_str = self.read_brace_modifier(chars);
            let (parts, may_glob) = self.expand_parts(&mod_str, span)?;
            let modifier_word = Word {
                parts,
                span,
                may_glob,
            };
            return Ok(WordPart::VarExpand(VarExpand {
                var,
                op: VarExpandOp::IndexDefault(idx),
                modifier: Some(Box::new(modifier_word)),
                span,
            }));
        }

        // Check for an operator or closing brace.
        let op = match chars.peek().map(|&(_, c)| c) {
            Some('}') => {
                chars.next();
                return Ok(WordPart::Var(var));
            }
            Some(':') => {
                chars.next();
                // `${var: -2}` — bash allows a space before a negative
                // offset: that space makes `-` an offset, not a `:-`
                // default operator.
                if chars.peek().map(|&(_, c)| c) == Some(' ') {
                    let mut probe = chars.clone();
                    probe.next();
                    if probe
                        .peek()
                        .map(|&(_, c)| c)
                        .map(|c| c.is_ascii_digit() || c == '-')
                        .unwrap_or(false)
                    {
                        chars.next(); // consume the space
                        let mut spec = String::new();
                        while let Some(&(_, c)) = chars.peek() {
                            if c == '}' {
                                break;
                            }
                            spec.push(c);
                            chars.next();
                        }
                        if chars.peek().map(|&(_, c)| c) == Some('}') {
                            chars.next();
                        }
                        return Ok(WordPart::VarExpand(VarExpand {
                            var,
                            op: VarExpandOp::Substring,
                            modifier: Some(Box::new(Word::literal(spec, span))),
                            span,
                        }));
                    }
                }
                match chars.peek().map(|&(_, c)| c) {
                    Some('-') => {
                        chars.next();
                        VarExpandOp::Default
                    }
                    Some('=') => {
                        chars.next();
                        VarExpandOp::Assign
                    }
                    Some('?') => {
                        chars.next();
                        VarExpandOp::Error
                    }
                    Some('+') => {
                        chars.next();
                        VarExpandOp::Alt
                    }
                    Some(c) if c.is_ascii_digit() || c == '-' => {
                        let mut spec = String::new();
                        while let Some(&(_, c)) = chars.peek() {
                            if c == '}' {
                                break;
                            }
                            spec.push(c);
                            chars.next();
                        }
                        if chars.peek().map(|&(_, c)| c) == Some('}') {
                            chars.next();
                        }
                        return Ok(WordPart::VarExpand(VarExpand {
                            var,
                            op: VarExpandOp::Substring,
                            modifier: Some(Box::new(Word::literal(spec, span))),
                            span,
                        }));
                    }
                    _ => return Err(ShellError::not_implemented("variable expansion operator")),
                }
            }
            Some('#') => {
                chars.next();
                let greedy = chars.peek().map(|&(_, c)| c) == Some('#');
                if greedy {
                    chars.next();
                }
                let pattern = self.read_brace_modifier(chars);
                let (parts, may_glob) = self.expand_parts(&pattern, span)?;
                let modifier_word = Word {
                    parts,
                    span,
                    may_glob,
                };
                return Ok(WordPart::VarExpand(VarExpand {
                    var,
                    op: if greedy {
                        VarExpandOp::TrimPrefixAll
                    } else {
                        VarExpandOp::TrimPrefix
                    },
                    modifier: Some(Box::new(modifier_word)),
                    span,
                }));
            }
            Some('%') => {
                chars.next();
                let greedy = chars.peek().map(|&(_, c)| c) == Some('%');
                if greedy {
                    chars.next();
                }
                let pattern = self.read_brace_modifier(chars);
                let (parts, may_glob) = self.expand_parts(&pattern, span)?;
                let modifier_word = Word {
                    parts,
                    span,
                    may_glob,
                };
                return Ok(WordPart::VarExpand(VarExpand {
                    var,
                    op: if greedy {
                        VarExpandOp::TrimSuffixAll
                    } else {
                        VarExpandOp::TrimSuffix
                    },
                    modifier: Some(Box::new(modifier_word)),
                    span,
                }));
            }
            // Colon-less forms: ${v-def} ${v=alt} ${v?msg} ${v+alt} —
            // test unset only (empty counts as set), unlike the `:` forms.
            Some('-') => {
                chars.next();
                VarExpandOp::DefaultUnset
            }
            Some('=') => {
                chars.next();
                VarExpandOp::AssignUnset
            }
            Some('?') => {
                chars.next();
                VarExpandOp::ErrorUnset
            }
            Some('+') => {
                chars.next();
                VarExpandOp::AltUnset
            }
            _ => return Err(ShellError::parse(span, "expected '}' or operator in ${}")),
        };

        // Collect the modifier word up to the matching '}'.
        let mod_str = self.read_brace_modifier(chars);
        let (parts, may_glob) = self.expand_parts(&mod_str, span)?;
        let modifier_word = Word {
            parts,
            span,
            may_glob,
        };

        Ok(WordPart::VarExpand(VarExpand {
            var,
            op,
            modifier: Some(Box::new(modifier_word)),
            span,
        }))
    }

    /// Collect raw chars for a brace modifier up to and including the closing `}`.
    fn read_brace_modifier(
        &self,
        chars: &mut std::iter::Peekable<std::str::CharIndices>,
    ) -> String {
        let mut buf = String::new();
        let mut depth = 1usize;
        for (_, c) in chars.by_ref() {
            match c {
                '{' => {
                    depth += 1;
                    buf.push(c);
                }
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    buf.push(c);
                }
                c => buf.push(c),
            }
        }
        buf
    }

    // ── Compound statements ────────────────────────────────────────────────────

    fn parse_if(&mut self) -> Result<Stmt, ShellError> {
        let start = self.stream.advance_span();
        self.stream.skip_newlines();
        let condition = self.parse_compound_list_until(&[TokenKind::Then])?;
        self.stream.expect(&TokenKind::Then)?;
        self.stream.skip_terminators();
        let then_body =
            self.parse_compound_list_until(&[TokenKind::Else, TokenKind::Elif, TokenKind::Fi])?;
        let else_body = match self.stream.peek_kind() {
            TokenKind::Else => {
                self.stream.advance();
                self.stream.skip_terminators();
                Some(self.parse_compound_list_until(&[TokenKind::Fi])?)
            }
            TokenKind::Elif => Some(vec![self.parse_elif()?]),
            _ => None,
        };
        let span = start.merge(self.stream.expect(&TokenKind::Fi)?);
        Ok(Stmt::If(IfStmt {
            condition,
            then_body,
            else_body,
            span,
        }))
    }

    fn parse_elif(&mut self) -> Result<Stmt, ShellError> {
        let start = self.stream.advance_span();
        self.stream.skip_newlines();
        let condition = self.parse_compound_list_until(&[TokenKind::Then])?;
        self.stream.expect(&TokenKind::Then)?;
        self.stream.skip_terminators();
        let then_body =
            self.parse_compound_list_until(&[TokenKind::Else, TokenKind::Elif, TokenKind::Fi])?;
        let else_body = match self.stream.peek_kind() {
            TokenKind::Else => {
                self.stream.advance();
                self.stream.skip_terminators();
                Some(self.parse_compound_list_until(&[TokenKind::Fi])?)
            }
            TokenKind::Elif => Some(vec![self.parse_elif()?]),
            _ => None,
        };
        Ok(Stmt::If(IfStmt {
            condition,
            then_body,
            else_body,
            span: start.merge(self.stream.current_span()),
        }))
    }

    fn parse_while(&mut self) -> Result<Stmt, ShellError> {
        let negated = matches!(self.stream.peek_kind(), TokenKind::Until);
        let start = self.stream.advance_span();
        self.stream.skip_newlines();
        let condition = self.parse_compound_list_until(&[TokenKind::Do])?;
        self.stream.expect(&TokenKind::Do)?;
        self.stream.skip_terminators();
        let body = self.parse_compound_list_until(&[TokenKind::Done])?;
        let span = start.merge(self.stream.expect(&TokenKind::Done)?);
        let mut redirects = vec![];
        while self.is_redirect_start() {
            redirects.push(self.parse_redirect()?);
        }
        self.attach_loop_heredoc_bodies(&mut redirects)?;
        Ok(Stmt::While(WhileStmt {
            condition,
            body,
            negated,
            redirects,
            span,
        }))
    }

    /// Consume queued heredoc-body tokens and attach them to the matching
    /// heredoc redirects collected after a loop's `done`.
    fn attach_loop_heredoc_bodies(
        &mut self,
        redirects: &mut Vec<Redirect>,
    ) -> Result<(), ShellError> {
        let mut next = 0usize;
        while let TokenKind::HereDocBody(body) = self.stream.peek_kind().clone() {
            // A body queued here can belong to an *earlier* command — e.g.
            // `cat <<EOF | while …; done` queues EOF's body after `done`.
            // Only consume when this loop itself owns an unfilled heredoc;
            // otherwise leave the token for the pipeline-level attach.
            if !redirects
                .iter()
                .any(|r| matches!(&r.target, RedirectTarget::HereDoc(s) if s.is_empty()))
            {
                return Ok(());
            }
            self.stream.advance();
            let idx = redirects
                .iter()
                .skip(next)
                .position(|r| matches!(r.target, RedirectTarget::HereDoc(_)))
                .map(|i| i + next)
                .ok_or_else(|| {
                    ShellError::parse(
                        self.stream.current_span(),
                        "unexpected heredoc body after loop",
                    )
                })?;
            redirects[idx].target = RedirectTarget::HereDoc(body);
            next = idx + 1;
        }
        Ok(())
    }

    fn parse_for(&mut self) -> Result<Stmt, ShellError> {
        let start = self.stream.advance_span();
        self.stream.skip_newlines();
        let var = match self.stream.peek_kind().clone() {
            TokenKind::Word(s) => {
                self.stream.advance();
                s
            }
            _ => {
                return Err(ShellError::parse(
                    self.stream.current_span(),
                    "expected variable name after 'for'",
                ))
            }
        };
        self.stream.skip_newlines();
        self.stream.expect(&TokenKind::In)?;
        let mut items = vec![];
        loop {
            match self.stream.peek_kind() {
                TokenKind::Newline | TokenKind::Semi | TokenKind::Do | TokenKind::Eof => break,
                TokenKind::Word(_) | TokenKind::SingleQuoted(_) | TokenKind::DoubleQuoted(_) => {
                    items.push(self.parse_word()?);
                }
                _ => break,
            }
        }
        self.stream.skip_terminators();
        self.stream.expect(&TokenKind::Do)?;
        self.stream.skip_terminators();
        let body = self.parse_compound_list_until(&[TokenKind::Done])?;
        let span = start.merge(self.stream.expect(&TokenKind::Done)?);
        let mut redirects = vec![];
        while self.is_redirect_start() {
            redirects.push(self.parse_redirect()?);
        }
        self.attach_loop_heredoc_bodies(&mut redirects)?;
        Ok(Stmt::For(ForStmt {
            var,
            items,
            body,
            redirects,
            span,
        }))
    }

    fn parse_case(&mut self) -> Result<Stmt, ShellError> {
        let start = self.stream.advance_span();
        self.stream.skip_newlines();
        let word = self.parse_word()?;
        self.stream.skip_newlines();
        self.stream.expect(&TokenKind::In)?;
        self.stream.skip_terminators();
        let mut arms = vec![];
        loop {
            self.stream.skip_terminators();
            if matches!(self.stream.peek_kind(), TokenKind::Esac | TokenKind::Eof) {
                break;
            }
            self.stream.eat(&TokenKind::LParen);
            let mut patterns = vec![];
            loop {
                let mut word = self.parse_word()?;
                // Merge glued fragments (`"g"*d*` = quoted + glob parts).
                while let Some(prev_end) = self.stream.prev_token().map(|t| t.span.end) {
                    if !matches!(
                        self.stream.peek_kind(),
                        TokenKind::Word(_)
                            | TokenKind::SingleQuoted(_)
                            | TokenKind::DoubleQuoted(_)
                    ) {
                        break;
                    }
                    if self.stream.peek().span.start != prev_end {
                        break;
                    }
                    let next = self.parse_word()?;
                    word.span = word.span.merge(next.span);
                    word.may_glob |= next.may_glob;
                    word.parts.extend(next.parts);
                }
                patterns.push(word);
                if self.stream.eat(&TokenKind::Pipe).is_some() {
                    continue;
                }
                break;
            }
            self.stream.expect(&TokenKind::RParen)?;
            self.stream.skip_terminators();
            let body = self.parse_case_arm_body()?;
            let arm_span = patterns[0].span;
            arms.push(CaseArm {
                patterns,
                body,
                span: arm_span,
            });
        }
        let span = start.merge(self.stream.expect(&TokenKind::Esac)?);
        Ok(Stmt::Case(CaseStmt { word, arms, span }))
    }

    fn parse_case_arm_body(&mut self) -> Result<Vec<Stmt>, ShellError> {
        let mut stmts = vec![];
        loop {
            self.stream.skip_newlines();
            match self.stream.peek_kind() {
                TokenKind::Esac | TokenKind::Eof => break,
                TokenKind::Semi => {
                    self.stream.advance();
                    if matches!(self.stream.peek_kind(), TokenKind::Semi) {
                        self.stream.advance();
                    }
                    break;
                }
                _ => {}
            }
            let stmt = self.parse_bool_chain(Prec::None)?;
            stmts.push(stmt);
            self.stream.skip_newlines();
            if matches!(self.stream.peek_kind(), TokenKind::Semi) {
                if !matches!(self.stream.peek_at(1).kind, TokenKind::Semi) {
                    self.stream.advance();
                }
            }
        }
        Ok(stmts)
    }

    fn parse_function_keyword(&mut self) -> Result<Stmt, ShellError> {
        let start = self.stream.advance_span();
        self.stream.skip_newlines();
        let name = match self.stream.peek_kind().clone() {
            TokenKind::Word(s) => {
                self.stream.advance();
                s
            }
            _ => {
                return Err(ShellError::parse(
                    self.stream.current_span(),
                    "expected function name",
                ))
            }
        };
        self.stream.eat(&TokenKind::LParen);
        self.stream.eat(&TokenKind::RParen);
        self.stream.skip_terminators();
        let (body, redirects) = self.parse_function_body()?;
        let span = start.merge(self.stream.current_span());
        Ok(Stmt::Function(FunctionDef {
            name,
            body,
            span,
            redirects,
        }))
    }

    fn parse_function_posix(&mut self) -> Result<Stmt, ShellError> {
        let start = self.stream.current_span();
        let name = match self.stream.peek_kind().clone() {
            TokenKind::Word(s) => {
                self.stream.advance();
                s
            }
            _ => return Err(ShellError::parse(start, "expected function name")),
        };
        // A word with '=' or '+' is an assignment, not a function name —
        // `arr=(` opens an array literal, `name=value` is a scalar.
        if name.contains('=') || name.contains('+') {
            return Err(ShellError::parse(start, "invalid function name"));
        }
        self.stream.expect(&TokenKind::LParen)?;
        self.stream.expect(&TokenKind::RParen)?;
        self.stream.skip_terminators();
        let (body, redirects) = self.parse_function_body()?;
        let span = start.merge(self.stream.current_span());
        Ok(Stmt::Function(FunctionDef {
            name,
            body,
            span,
            redirects,
        }))
    }

    fn parse_function_body(&mut self) -> Result<(Vec<Stmt>, Vec<Redirect>), ShellError> {
        if self.stream.eat(&TokenKind::LBrace).is_some() {
            self.stream.skip_terminators();
            let stmts = self.parse_compound_list_until(&[TokenKind::RBrace])?;
            self.stream.expect(&TokenKind::RBrace)?;
            let mut redirects = vec![];
            while self.is_redirect_start() {
                redirects.push(self.parse_redirect()?);
            }
            Ok((stmts, redirects))
        } else {
            Ok((vec![self.parse_stmt()?], vec![]))
        }
    }

    fn parse_brace_group(&mut self) -> Result<Stmt, ShellError> {
        self.stream.advance();
        self.stream.skip_terminators();
        let stmts = self.parse_compound_list_until(&[TokenKind::RBrace])?;
        self.stream.expect(&TokenKind::RBrace)?;
        let mut redirects = vec![];
        while self.is_redirect_start() {
            redirects.push(self.parse_redirect()?);
        }
        Ok(Stmt::BraceGroup { stmts, redirects })
    }

    fn parse_subshell(&mut self) -> Result<Stmt, ShellError> {
        self.stream.advance();
        self.stream.skip_terminators();
        let stmts = self.parse_compound_list_until(&[TokenKind::RParen])?;
        self.stream.expect(&TokenKind::RParen)?;
        let mut redirects = vec![];
        while self.is_redirect_start() {
            redirects.push(self.parse_redirect()?);
        }
        Ok(Stmt::Subshell { stmts, redirects })
    }

    fn parse_compound_list_until(&mut self, stop: &[TokenKind]) -> Result<Vec<Stmt>, ShellError> {
        let mut stmts = vec![];
        loop {
            self.stream.skip_terminators();
            if self.stream.is_eof() {
                let expected: Vec<String> = stop.iter().map(|k| k.to_string()).collect();
                return Err(ShellError::parse(
                    self.stream.current_span(),
                    &format!(
                        "unexpected end of input, expected {}",
                        expected.join(" or ")
                    ),
                ));
            }
            if stop.iter().any(|k| k == self.stream.peek_kind()) {
                break;
            }
            stmts.push(self.parse_stmt()?);
        }
        Ok(stmts)
    }

    /// Brace expansion inside an array literal — same protocol as
    /// try_brace_expand_in_command but growing the element list.
    fn try_brace_expand_in_elems(
        &mut self,
        elems: &mut Vec<Word>,
    ) -> Result<Option<Vec<Word>>, ShellError> {
        let span = self.stream.current_span();
        let mut depth = 1;
        let mut offset = 1;
        let mut body = String::new();
        loop {
            let tok = self.stream.peek_at(offset);
            match &tok.kind {
                TokenKind::LBrace => {
                    depth += 1;
                    body.push('{');
                    offset += 1;
                }
                TokenKind::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    body.push('}');
                    offset += 1;
                }
                TokenKind::Word(s) => {
                    body.push_str(s);
                    offset += 1;
                }
                TokenKind::SingleQuoted(s) => {
                    body.push_str(s);
                    offset += 1;
                }
                TokenKind::DoubleQuoted(s) => {
                    body.push('"');
                    body.push_str(s);
                    body.push('"');
                    offset += 1;
                }
                _ => return Ok(None),
            }
        }

        let lbrace_span = self.stream.peek().span;
        let prefix_parts: Vec<WordPart> = if let Some(prev) = self.stream.prev_token() {
            if prev.span.end == lbrace_span.start
                && matches!(
                    &prev.kind,
                    TokenKind::Word(_) | TokenKind::SingleQuoted(_) | TokenKind::DoubleQuoted(_)
                )
            {
                elems.last().map(|w| w.parts.clone()).unwrap_or_default()
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        let rbrace_span = self.stream.peek_at(offset).span;
        let suffix_tok = self.stream.peek_at(offset + 1);
        let suffix_parts: Vec<WordPart> = if rbrace_span.end == suffix_tok.span.start {
            match &suffix_tok.kind {
                TokenKind::Word(s) => vec![WordPart::Literal(s.clone())],
                TokenKind::SingleQuoted(s) => vec![WordPart::Literal(s.clone())],
                _ => vec![],
            }
        } else {
            vec![]
        };

        let expanded =
            shell_ast::brace_expand::try_brace_expand(&prefix_parts, &body, &suffix_parts, span);
        if expanded.is_none() {
            return Ok(None);
        }
        let expanded = expanded.unwrap();

        self.stream.advance(); // LBrace
        for _ in 0..offset - 1 {
            self.stream.advance();
        }
        self.stream.advance(); // RBrace
        if !suffix_parts.is_empty() {
            self.stream.advance();
        }
        if !prefix_parts.is_empty() {
            elems.pop();
        }

        let expanded = self.expand_adjacent_braces(expanded)?;
        Ok(Some(expanded))
    }

    /// After one brace group was expanded and consumed, check for directly
    /// adjacent further groups (`{x,y}{1,2}`) and fold them in. Each word in
    /// `expanded` becomes the prefix for the next group.
    fn expand_adjacent_braces(&mut self, mut expanded: Vec<Word>) -> Result<Vec<Word>, ShellError> {
        loop {
            // Adjacent LBrace with no separating whitespace?
            let adjacent = match self.stream.prev_token() {
                Some(prev) => match self.stream.peek_kind() {
                    TokenKind::LBrace => prev.span.end == self.stream.peek().span.start,
                    _ => false,
                },
                None => false,
            };
            if !adjacent {
                return Ok(expanded);
            }
            let span = self.stream.current_span();
            let mut depth = 1;
            let mut offset = 1;
            let mut body = String::new();
            loop {
                let tok = self.stream.peek_at(offset);
                match &tok.kind {
                    TokenKind::LBrace => {
                        depth += 1;
                        body.push('{');
                        offset += 1;
                    }
                    TokenKind::RBrace => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                        body.push('}');
                        offset += 1;
                    }
                    TokenKind::Word(s) => {
                        body.push_str(s);
                        offset += 1;
                    }
                    TokenKind::SingleQuoted(s) => {
                        body.push_str(s);
                        offset += 1;
                    }
                    TokenKind::DoubleQuoted(s) => {
                        body.push('"');
                        body.push_str(s);
                        body.push('"');
                        offset += 1;
                    }
                    _ => return Ok(expanded),
                }
            }
            // Suffix glued to this group's `}` (e.g. `{1,2}.txt`).
            let rbrace_span = self.stream.peek_at(offset).span;
            let suffix_tok = self.stream.peek_at(offset + 1);
            let suffix_parts: Vec<WordPart> = if rbrace_span.end == suffix_tok.span.start {
                match &suffix_tok.kind {
                    TokenKind::Word(s) => vec![WordPart::Literal(s.clone())],
                    TokenKind::SingleQuoted(s) => vec![WordPart::Literal(s.clone())],
                    _ => vec![],
                }
            } else {
                vec![]
            };

            let mut combined: Vec<Word> = vec![];
            for pre in expanded.iter() {
                if let Some(mut words) = shell_ast::brace_expand::try_brace_expand(
                    &pre.parts,
                    &body,
                    &suffix_parts,
                    span,
                ) {
                    combined.append(&mut words);
                }
            }
            if combined.is_empty() {
                return Ok(expanded);
            }

            // Consume the group + suffix.
            self.stream.advance(); // LBrace
            for _ in 0..offset - 1 {
                self.stream.advance();
            }
            self.stream.advance(); // RBrace
            if !suffix_parts.is_empty() {
                self.stream.advance();
            }
            expanded = combined;
        }
    }

    fn try_brace_expand_in_command(
        &mut self,
        argv: &mut Vec<Word>,
    ) -> Result<Option<Vec<Word>>, ShellError> {
        let span = self.stream.current_span();
        let mut depth = 1;
        let mut offset = 1;
        let mut body = String::new();
        loop {
            let tok = self.stream.peek_at(offset);
            match &tok.kind {
                TokenKind::LBrace => {
                    depth += 1;
                    body.push('{');
                    offset += 1;
                }
                TokenKind::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    body.push('}');
                    offset += 1;
                }
                TokenKind::Word(s) => {
                    body.push_str(s);
                    offset += 1;
                }
                TokenKind::SingleQuoted(s) => {
                    body.push_str(s);
                    offset += 1;
                }
                TokenKind::DoubleQuoted(s) => {
                    body.push('"');
                    body.push_str(s);
                    body.push('"');
                    offset += 1;
                }
                _ => return Ok(None),
            }
        }

        let lbrace_span = self.stream.peek().span;
        let prefix_parts: Vec<WordPart> = if let Some(prev) = self.stream.prev_token() {
            if prev.span.end == lbrace_span.start
                && matches!(
                    &prev.kind,
                    TokenKind::Word(_) | TokenKind::SingleQuoted(_) | TokenKind::DoubleQuoted(_)
                )
            {
                argv.last().map(|w| w.parts.clone()).unwrap_or_default()
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        let rbrace_span = self.stream.peek_at(offset).span;
        let suffix_tok = self.stream.peek_at(offset + 1);
        let suffix_parts: Vec<WordPart> = if rbrace_span.end == suffix_tok.span.start {
            match &suffix_tok.kind {
                TokenKind::Word(s) => vec![WordPart::Literal(s.clone())],
                TokenKind::SingleQuoted(s) => vec![WordPart::Literal(s.clone())],
                _ => vec![],
            }
        } else {
            vec![]
        };

        let expanded =
            shell_ast::brace_expand::try_brace_expand(&prefix_parts, &body, &suffix_parts, span);
        if expanded.is_none() {
            return Ok(None);
        }
        let expanded = expanded.unwrap();

        self.stream.advance(); // LBrace
        for _ in 0..offset - 1 {
            self.stream.advance();
        }
        self.stream.advance(); // RBrace

        if !suffix_parts.is_empty() {
            self.stream.advance();
        }

        if !prefix_parts.is_empty() {
            argv.pop();
        }

        let expanded = self.expand_adjacent_braces(expanded)?;
        Ok(Some(expanded))
    }

    fn consume_brace_as_literal(&mut self) -> String {
        let mut s = String::from("{");
        self.stream.advance(); // consume LBrace
        loop {
            match &self.stream.peek().kind {
                TokenKind::RBrace => {
                    s.push('}');
                    self.stream.advance();
                    break;
                }
                TokenKind::Word(w) => {
                    s.push_str(w);
                    self.stream.advance();
                }
                TokenKind::SingleQuoted(w) => {
                    s.push_str(w);
                    self.stream.advance();
                }
                TokenKind::DoubleQuoted(w) => {
                    s.push('"');
                    s.push_str(w);
                    s.push('"');
                    self.stream.advance();
                }
                TokenKind::LBrace => {
                    s.push('{');
                    self.stream.advance();
                }
                _ => break,
            }
        }
        s
    }
}

// ── Free functions ─────────────────────────────────────────────────────────────

/// Read body of `$(( ... ))` — caller has already consumed `((`; stops at `))`.
fn read_arith_body(chars: &mut std::iter::Peekable<std::str::CharIndices>) -> String {
    let mut inner = String::new();
    let mut depth = 0usize; // extra paren depth inside the expression
    loop {
        match chars.peek().map(|&(_, c)| c) {
            None => break,
            Some('(') => {
                chars.next();
                depth += 1;
                inner.push('(');
            }
            Some(')') => {
                if depth == 0 {
                    chars.next(); // first )
                                  // consume second )
                    if chars.peek().map(|&(_, c)| c) == Some(')') {
                        chars.next();
                    }
                    break;
                }
                chars.next();
                depth -= 1;
                inner.push(')');
            }
            Some(c) => {
                chars.next();
                inner.push(c);
            }
        }
    }
    inner
}

/// Read body of `$( ... )` — caller has already consumed `(`; stops at matching `)`.
fn read_cmdsub_body(chars: &mut std::iter::Peekable<std::str::CharIndices>) -> String {
    let mut inner = String::new();
    let mut depth = 1usize;
    let mut in_case = 0usize;
    let mut word_buf = String::new();

    loop {
        match chars.next() {
            None => break,
            Some((_, '(')) => {
                cmdsub_flush_word(&word_buf, &mut in_case);
                word_buf.clear();
                depth += 1;
                inner.push('(');
            }
            Some((_, ')')) => {
                cmdsub_flush_word(&word_buf, &mut in_case);
                word_buf.clear();
                if in_case > 0 && depth == 1 {
                    // Case-pattern terminator, not the outer $() closer.
                    inner.push(')');
                } else {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                    inner.push(')');
                }
            }
            Some((_, c)) if c.is_alphanumeric() || c == '_' => {
                word_buf.push(c);
                inner.push(c);
            }
            Some((_, c)) => {
                cmdsub_flush_word(&word_buf, &mut in_case);
                word_buf.clear();
                inner.push(c);
            }
        }
    }
    inner
}

#[inline]
fn cmdsub_flush_word(word: &str, in_case: &mut usize) {
    match word {
        "case" => *in_case += 1,
        "esac" => {
            if *in_case > 0 {
                *in_case -= 1;
            }
        }
        _ => {}
    }
}

fn is_special_var(c: char) -> bool {
    matches!(c, '?' | '#' | '*' | '@' | '$' | '!' | '-') || c.is_ascii_digit()
}

fn is_valid_var_name(s: &str) -> bool {
    !s.is_empty()
        && s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn bash_word_is_terminator(s: &str) -> bool {
    // Words that signal the end of a bash-word in [[ ]] context
    s == "]]" || s == "!" || is_bash_binary_op(s) || is_bash_unary_op(s)
}

fn is_bash_unary_op(s: &str) -> bool {
    matches!(
        s,
        "-f" | "-d"
            | "-e"
            | "-r"
            | "-w"
            | "-x"
            | "-s"
            | "-L"
            | "-h"
            | "-p"
            | "-b"
            | "-c"
            | "-k"
            | "-g"
            | "-u"
            | "-t"
            | "-O"
            | "-G"
            | "-S"
            | "-z"
            | "-n"
    )
}

fn is_bash_binary_op(s: &str) -> bool {
    matches!(
        s,
        "=" | "=="
            | "!="
            | "=~"
            | "<"
            | ">"
            | "-eq"
            | "-ne"
            | "-lt"
            | "-le"
            | "-gt"
            | "-ge"
            | "-nt"
            | "-ot"
            | "-ef"
    )
}

/// argv is exactly one word naming a variable-declaration builtin.
fn local_decl_cmd(argv: &[Word]) -> bool {
    if argv.len() != 1 {
        return false;
    }
    matches!(
        argv[0].as_literal(),
        Some("local") | Some("declare") | Some("typeset") | Some("readonly")
    )
}

fn try_parse_assignment(word: &Word) -> Option<Assignment> {
    let lit = match word.parts.first()? {
        WordPart::Literal(s) => s,
        _ => return None,
    };

    // arr[$i]=v — the index is an expansion, so the word arrived as
    // [Literal("arr["), Var("i"), Literal("]=v")]. Flatten to a raw
    // string first, then run the index-assignment matcher on it.
    if word.parts.len() > 1 && lit.ends_with('[') {
        let flattened = flatten_word_raw(word);
        if let Some((a, _)) = parse_index_from_str(&flattened, word) {
            return Some(a);
        }
        return None;
    }

    // arr[i]=v — element assignment; index may be digits, @, *, or $var.
    if let Some(a) = try_parse_index_assignment(word, lit) {
        return Some(a);
    }

    // arr+=( — array append; the `(` starts the element list.
    if let Some(eq) = lit.find("+=") {
        let name = &lit[..eq];
        if is_valid_var_name(name) && word.parts.len() == 1 {
            let after = &lit[eq + 2..];
            if after.is_empty() {
                return Some(Assignment {
                    name: name.to_string(),
                    value: vec![],
                    span: word.span,
                    is_array: true,
                    append: true,
                    index: None,
                    elements: vec![],
                    local_decl: false,
                });
            }
        }
        return None;
    }

    let eq = lit.find('=')?;
    let name = &lit[..eq];
    if !is_valid_var_name(name) {
        return None;
    }
    let after = &lit[eq + 1..];
    let mut value: Vec<WordPart> = vec![];
    if !after.is_empty() {
        value.push(WordPart::Literal(after.to_string()));
    }
    for part in &word.parts[1..] {
        value.push(part.clone());
    }
    Some(Assignment::scalar(name.to_string(), value, word.span))
}

/// Flatten a word to raw text for LHS matching: Var(v) becomes `$v`,
/// literals pass through. Only used for assignment-head detection.
fn flatten_word_raw(word: &Word) -> String {
    let mut s = String::new();
    for p in &word.parts {
        match p {
            WordPart::Literal(l) => s.push_str(l),
            WordPart::Var(v) => {
                s.push('$');
                s.push_str(v);
            }
            _ => {}
        }
    }
    s
}

/// Parse `name[IDX]=…` from a flattened head string. The value parts come
/// from the ORIGINAL word: the part holding `]=` is split at the `=`.
fn parse_index_from_str(flat: &str, word: &Word) -> Option<(Assignment, usize)> {
    let open = flat.find('[')?;
    let close = flat[open + 1..].find(']')? + open + 1;
    let eq = flat[close + 1..].find('=')? + close + 1;
    let name = &flat[..open];
    if !is_valid_var_name(name) {
        return None;
    }
    let idx = &flat[open + 1..close];
    let idx_ok = idx == "@"
        || idx == "*"
        || (!idx.is_empty() && idx.chars().all(|c| c.is_ascii_digit()))
        || idx.starts_with('$');
    if !idx_ok {
        return None;
    }

    // Locate the part in the original word containing `]=…` and split it.
    let mut value: Vec<WordPart> = vec![];
    let mut parts_iter = word.parts.iter().enumerate();
    'outer: for (_pi, part) in parts_iter.by_ref() {
        if let WordPart::Literal(l) = part {
            if let Some(rel) = l.find("]=") {
                let after = &l[rel + 2..];
                if !after.is_empty() {
                    value.push(WordPart::Literal(after.to_string()));
                }
                // Remaining parts belong to the value.
                for (_, p) in parts_iter {
                    value.push(p.clone());
                }
                break 'outer;
            }
        }
    }

    let a = Assignment {
        name: name.to_string(),
        value,
        span: word.span,
        is_array: false,
        append: false,
        index: Some(idx.to_string()),
        elements: vec![],
        local_decl: false,
    };
    Some((a, eq))
}

/// Match `name[IDX]=value…` on the word's leading literal. Returns an
/// index-assignment whose `value` holds the RHS parts.
fn try_parse_index_assignment(word: &Word, lit: &str) -> Option<Assignment> {
    let open = lit.find('[')?;
    let close = lit[open + 1..].find(']')? + open + 1;
    let eq = lit[close + 1..].find('=')? + close + 1;
    let name = &lit[..open];
    if !is_valid_var_name(name) {
        return None;
    }
    let idx = &lit[open + 1..close];
    // Accept digits, @/*, or a $-expansion ($i, $((…)) — evaluated later).
    let idx_ok = idx == "@"
        || idx == "*"
        || !idx.is_empty() && idx.chars().all(|c| c.is_ascii_digit())
        || idx.starts_with('$');
    if !idx_ok {
        return None;
    }
    let after = &lit[eq + 1..];
    let mut value: Vec<WordPart> = vec![];
    if !after.is_empty() {
        value.push(WordPart::Literal(after.to_string()));
    }
    for part in &word.parts[1..] {
        value.push(part.clone());
    }
    Some(Assignment {
        name: name.to_string(),
        value,
        span: word.span,
        is_array: false,
        append: false,
        index: Some(idx.to_string()),
        elements: vec![],
        local_decl: false,
    })
}
