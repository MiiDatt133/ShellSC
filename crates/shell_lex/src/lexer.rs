use std::collections::VecDeque;

use crate::{
    cursor::Cursor,
    token::{Token, TokenKind},
};
use shell_ast::{ShellError, Span};

#[derive(Debug, Clone)]
struct PendingHereDoc {
    delimiter: String,
    strip_tabs: bool,
}

pub struct Lexer<'a> {
    cursor: Cursor<'a>,
    queued: VecDeque<Token>,
    awaiting_heredoc: Option<bool>,
    pending_heredocs: Vec<PendingHereDoc>,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            cursor: Cursor::new(src),
            queued: VecDeque::new(),
            awaiting_heredoc: None,
            pending_heredocs: vec![],
        }
    }

    pub fn tokenize(mut self) -> Result<Vec<Token>, ShellError> {
        let mut tokens = vec![];
        loop {
            let tok = self.next_token()?;
            let eof = tok.kind == TokenKind::Eof;
            tokens.push(tok);
            if eof {
                break;
            }
        }
        Ok(tokens)
    }

    fn next_token(&mut self) -> Result<Token, ShellError> {
        if let Some(tok) = self.queued.pop_front() {
            return Ok(tok);
        }

        self.skip_whitespace_horizontal();

        let start = self.cursor.pos();
        let start_ln = self.cursor.line;
        let start_col = self.cursor.col;

        let c = match self.cursor.peek() {
            None => {
                if self.pending_heredocs.is_empty() {
                    return Ok(self.make(TokenKind::Eof, start, start_ln, start_col));
                }
                return Err(ShellError::lex(
                    Span::new(start, start, start_ln, start_col),
                    "unterminated heredoc",
                ));
            }
            Some(c) => c,
        };

        match c {
            '\n' => {
                if self.pending_heredocs.is_empty() {
                    self.cursor.advance();
                    Ok(self.make(TokenKind::Newline, start, start_ln, start_col))
                } else {
                    self.cursor.advance();
                    self.queue_heredoc_bodies(start, start_ln, start_col)?;
                    self.queued.pop_front().ok_or_else(|| {
                        ShellError::lex(
                            Span::new(start, self.cursor.pos(), start_ln, start_col),
                            "internal heredoc queue error",
                        )
                    })
                }
            }
            '#' => {
                self.skip_comment();
                if self.pending_heredocs.is_empty() {
                    Ok(Token::new(
                        TokenKind::Newline,
                        self.cursor.span_from(start, start_ln, start_col),
                    ))
                } else {
                    if self.cursor.peek() == Some('\n') {
                        self.cursor.advance();
                    }
                    self.queue_heredoc_bodies(start, start_ln, start_col)?;
                    self.queued.pop_front().ok_or_else(|| {
                        ShellError::lex(
                            Span::new(start, self.cursor.pos(), start_ln, start_col),
                            "internal heredoc queue error",
                        )
                    })
                }
            }
            '\'' => {
                self.cursor.advance();
                self.lex_single_quoted(start_ln, start_col)
            }
            '"' => {
                self.cursor.advance();
                self.lex_double_quoted(start_ln, start_col)
            }
            '|' => {
                self.cursor.advance();
                if self.cursor.eat('|') {
                    Ok(self.make(TokenKind::Or, start, start_ln, start_col))
                } else {
                    Ok(self.make(TokenKind::Pipe, start, start_ln, start_col))
                }
            }
            '&' => {
                self.cursor.advance();
                if self.cursor.eat('&') {
                    Ok(self.make(TokenKind::And, start, start_ln, start_col))
                } else {
                    Ok(self.make(TokenKind::Ampersand, start, start_ln, start_col))
                }
            }
            ';' => {
                self.cursor.advance();
                Ok(self.make(TokenKind::Semi, start, start_ln, start_col))
            }
            '(' => {
                self.cursor.advance();
                Ok(self.make(TokenKind::LParen, start, start_ln, start_col))
            }
            ')' => {
                self.cursor.advance();
                Ok(self.make(TokenKind::RParen, start, start_ln, start_col))
            }
            '{' => {
                self.cursor.advance();
                Ok(self.make(TokenKind::LBrace, start, start_ln, start_col))
            }
            '}' => {
                self.cursor.advance();
                Ok(self.make(TokenKind::RBrace, start, start_ln, start_col))
            }
            '>' => {
                self.cursor.advance();
                if self.cursor.eat('>') {
                    Ok(self.make(TokenKind::RedirAppend, start, start_ln, start_col))
                } else if self.cursor.eat('&') {
                    Ok(self.make(TokenKind::RedirOutFd, start, start_ln, start_col))
                } else {
                    Ok(self.make(TokenKind::RedirOut, start, start_ln, start_col))
                }
            }
            '<' => {
                self.cursor.advance();
                if self.cursor.eat('<') {
                    if self.cursor.eat('<') {
                        Ok(self.make(TokenKind::HereString, start, start_ln, start_col))
                    } else {
                        let strip_tabs = self.cursor.eat('-');
                        self.awaiting_heredoc = Some(strip_tabs);
                        if strip_tabs {
                            Ok(self.make(TokenKind::HereDocStrip, start, start_ln, start_col))
                        } else {
                            Ok(self.make(TokenKind::HereDoc, start, start_ln, start_col))
                        }
                    }
                } else if self.cursor.eat('&') {
                    Ok(self.make(TokenKind::RedirInFd, start, start_ln, start_col))
                } else {
                    Ok(self.make(TokenKind::RedirIn, start, start_ln, start_col))
                }
            }
            c if c.is_ascii_digit() && self.is_fd_redirect_ahead() => {
                self.lex_fd_redirect(start, start_ln, start_col)
            }
            _ => self.lex_word(start, start_ln, start_col),
        }
    }

    fn skip_whitespace_horizontal(&mut self) {
        loop {
            match self.cursor.peek() {
                Some(' ') | Some('\t') | Some('\r') => {
                    self.cursor.advance();
                }
                _ => break,
            }
        }
    }

    fn skip_comment(&mut self) {
        loop {
            match self.cursor.peek() {
                None | Some('\n') => break,
                _ => {
                    self.cursor.advance();
                }
            }
        }
    }

    fn lex_single_quoted(&mut self, start_ln: u32, start_col: u32) -> Result<Token, ShellError> {
        let span_start = self.cursor.pos() - 1;
        let mut s = String::new();
        loop {
            match self.cursor.advance() {
                None => {
                    return Err(ShellError::lex(
                        Span::new(span_start, self.cursor.pos(), start_ln, start_col),
                        "unterminated single-quoted string",
                    ))
                }
                Some('\'') => break,
                Some(c) => s.push(c),
            }
        }
        let tok = Token::new(
            TokenKind::SingleQuoted(s),
            self.cursor.span_from(span_start, start_ln, start_col),
        );
        self.record_heredoc_delimiter(&tok.kind)?;
        Ok(tok)
    }

    fn lex_double_quoted(&mut self, start_ln: u32, start_col: u32) -> Result<Token, ShellError> {
        let span_start = self.cursor.pos() - 1;
        let mut s = String::new();
        loop {
            match self.cursor.peek() {
                None => {
                    return Err(ShellError::lex(
                        Span::new(span_start, self.cursor.pos(), start_ln, start_col),
                        "unterminated double-quoted string",
                    ))
                }
                Some('"') => {
                    self.cursor.advance();
                    break;
                }
                Some('$') => {
                    self.cursor.advance();
                    s.push('$');
                    if self.cursor.peek() == Some('(') {
                        self.cursor.advance();
                        s.push('(');
                        read_cmd_sub_body(&mut self.cursor, &mut s);
                    } else if self.cursor.peek() == Some('{') {
                        self.cursor.advance();
                        s.push('{');
                        let mut depth = 1usize;
                        loop {
                            match self.cursor.advance() {
                                None => break,
                                Some('{') => {
                                    depth += 1;
                                    s.push('{');
                                }
                                Some('}') => {
                                    depth -= 1;
                                    s.push('}');
                                    if depth == 0 {
                                        break;
                                    }
                                }
                                Some(c) => s.push(c),
                            }
                        }
                    }
                }
                Some('`') => {
                    self.cursor.advance();
                    s.push('`');
                    read_backtick_body(&mut self.cursor, &mut s);
                }
                Some('\\') => {
                    self.cursor.advance();
                    match self.cursor.peek() {
                        Some('"') | Some('\\') | Some('$') | Some('`') | Some('\n') => {
                            // Preserve the backslash so the parser can
                            // distinguish escaped quotes (`\"`) from the
                            // closing delimiter and keep `\$/\`` as literal
                            // triggers rather than expansions.
                            s.push('\\');
                            if let Some(c) = self.cursor.advance() {
                                if c != '\n' {
                                    s.push(c);
                                }
                            }
                        }
                        _ => {
                            // Non-special escape inside dquotes: keep both
                            // chars verbatim (bash behaviour).
                            s.push('\\');
                            if let Some(c) = self.cursor.advance() {
                                s.push(c);
                            }
                        }
                    }
                }
                Some(_) => {
                    s.push(self.cursor.advance().unwrap());
                }
            }
        }
        let tok = Token::new(
            TokenKind::DoubleQuoted(s),
            self.cursor.span_from(span_start, start_ln, start_col),
        );
        self.record_heredoc_delimiter(&tok.kind)?;
        Ok(tok)
    }

    fn lex_word(
        &mut self,
        start: usize,
        start_ln: u32,
        start_col: u32,
    ) -> Result<Token, ShellError> {
        let mut s = String::new();
        // Decoded $'...' text pending emission as its own SingleQuoted token.
        let mut ansi_standalone: Option<String> = None;
        let mut ansi_span_start: Option<usize> = None;
        loop {
            match self.cursor.peek() {
                None | Some('\n') | Some(' ') | Some('\t') | Some('\r') | Some(';') | Some('&')
                | Some('|') | Some('<') | Some('>') | Some('(') | Some(')') | Some('{')
                | Some('}') => break,

                Some('"') => {
                    self.cursor.advance();
                    s.push('"');
                    loop {
                        match self.cursor.peek() {
                            None => break,
                            Some('"') => {
                                self.cursor.advance();
                                s.push('"');
                                break;
                            }
                            Some('\\') => {
                                self.cursor.advance();
                                match self.cursor.advance() {
                                    Some(c) => {
                                        s.push('\\');
                                        s.push(c);
                                    }
                                    None => s.push('\\'),
                                }
                            }
                            Some('$') => {
                                self.cursor.advance();
                                s.push('$');
                                if self.cursor.peek() == Some('(') {
                                    self.cursor.advance();
                                    s.push('(');
                                    read_cmd_sub_body(&mut self.cursor, &mut s);
                                }
                            }
                            Some('`') => {
                                self.cursor.advance();
                                s.push('`');
                                read_backtick_body(&mut self.cursor, &mut s);
                            }
                            Some(_) => {
                                s.push(self.cursor.advance().unwrap());
                            }
                        }
                    }
                }

                Some('\'') => {
                    self.cursor.advance();
                    s.push('\'');
                    loop {
                        match self.cursor.advance() {
                            None => break,
                            Some('\'') => {
                                s.push('\'');
                                break;
                            }
                            Some(c) => s.push(c),
                        }
                    }
                }

                Some('$') => {
                    self.cursor.advance();
                    // ANSI-C quoting: $'...' — escapes decoded at lex time.
                    // The decoded text is emitted as its own SingleQuoted token
                    // so the parser keeps it literal (a Word would re-interpret
                    // embedded quotes/expansions).
                    if self.cursor.peek() == Some('\'') {
                        // Span starts at the `$` so the ANSI token stays
                        // adjacent to the previous token for merge purposes.
                        let ansi_start = self.cursor.pos() - 1;
                        self.cursor.advance();
                        let mut decoded = String::new();
                        read_ansi_c_body(&mut self.cursor, &mut decoded);
                        ansi_standalone = Some(decoded);
                        ansi_span_start = Some(ansi_start);
                        break;
                    } else {
                        s.push('$');
                        if self.cursor.peek() == Some('(') {
                            self.cursor.advance();
                            s.push('(');
                            read_cmd_sub_body(&mut self.cursor, &mut s);
                        } else if self.cursor.peek() == Some('{') {
                            // ${var}, ${var:-default}, ${#var}, etc. — read to matching '}'
                            self.cursor.advance();
                            s.push('{');
                            let mut depth = 1usize;
                            loop {
                                match self.cursor.advance() {
                                    None => break,
                                    Some('{') => {
                                        depth += 1;
                                        s.push('{');
                                    }
                                    Some('}') => {
                                        depth -= 1;
                                        s.push('}');
                                        if depth == 0 {
                                            break;
                                        }
                                    }
                                    Some(c) => s.push(c),
                                }
                            }
                        }
                    }
                }

                Some('`') => {
                    self.cursor.advance();
                    s.push('`');
                    read_backtick_body(&mut self.cursor, &mut s);
                }
                Some('#') => {
                    s.push(self.cursor.advance().unwrap());
                }

                Some('\\') => {
                    self.cursor.advance();
                    match self.cursor.peek() {
                        Some('\n') => {
                            self.cursor.advance();
                        }
                        Some(_) => {
                            s.push(self.cursor.advance().unwrap());
                        }
                        None => {
                            s.push('\\');
                        }
                    }
                }

                Some(_) => {
                    s.push(self.cursor.advance().unwrap());
                }
            }
        }

        // $'...' decoded: emit as SingleQuoted (literal — no re-interpretation).
        // With a preceding word fragment, the Word goes out first and the
        // ANSI text is queued for the next next_token call.
        if let Some(decoded) = ansi_standalone {
            let a_start = ansi_span_start.unwrap_or(start);
            let a_end = self.cursor.pos();
            let ansi_tok = Token::new(
                TokenKind::SingleQuoted(decoded),
                shell_ast::Span::new(a_start, a_end, start_ln, start_col),
            );
            self.record_heredoc_delimiter(&ansi_tok.kind)?;
            if s.is_empty() {
                return Ok(ansi_tok);
            }
            // The Word ends where $' began — the queued SingleQuoted starts
            // exactly there so the parser's adjacency merge joins them.
            let word_tok = Token::new(
                TokenKind::Word(s),
                shell_ast::Span::new(start, a_start, start_ln, start_col),
            );
            self.queued.push_back(ansi_tok);
            self.record_heredoc_delimiter(&word_tok.kind)?;
            return Ok(word_tok);
        }
        let span = self.cursor.span_from(start, start_ln, start_col);
        let tok = if self.awaiting_heredoc.is_some() {
            Token::new(TokenKind::Word(s), span)
        } else {
            Token::new(TokenKind::keyword_or_word(s), span)
        };
        self.record_heredoc_delimiter(&tok.kind)?;
        Ok(tok)
    }

    fn is_fd_redirect_ahead(&self) -> bool {
        matches!(self.cursor.peek2(), Some('>') | Some('<'))
    }

    fn lex_fd_redirect(
        &mut self,
        start: usize,
        start_ln: u32,
        start_col: u32,
    ) -> Result<Token, ShellError> {
        let mut s = String::new();
        while let Some(c) = self.cursor.peek() {
            if c.is_ascii_digit() {
                s.push(c);
                self.cursor.advance();
            } else {
                break;
            }
        }
        Ok(Token::new(
            TokenKind::Word(s),
            self.cursor.span_from(start, start_ln, start_col),
        ))
    }

    fn make(&self, kind: TokenKind, start: usize, ln: u32, col: u32) -> Token {
        Token::new(kind, self.cursor.span_from(start, ln, col))
    }

    fn record_heredoc_delimiter(&mut self, kind: &TokenKind) -> Result<(), ShellError> {
        let Some(strip_tabs) = self.awaiting_heredoc.take() else {
            return Ok(());
        };

        let delimiter = match kind {
            TokenKind::Word(s) => s.clone(),
            TokenKind::SingleQuoted(s) => s.clone(),
            TokenKind::DoubleQuoted(s) => s.clone(),
            other => {
                return Err(ShellError::lex(
                    Span::new(
                        self.cursor.pos(),
                        self.cursor.pos(),
                        self.cursor.line,
                        self.cursor.col,
                    ),
                    format!("invalid heredoc delimiter token: {}", other),
                ))
            }
        };

        self.pending_heredocs.push(PendingHereDoc {
            delimiter,
            strip_tabs,
        });
        Ok(())
    }

    fn queue_heredoc_bodies(
        &mut self,
        start: usize,
        start_ln: u32,
        start_col: u32,
    ) -> Result<(), ShellError> {
        let heredocs = std::mem::take(&mut self.pending_heredocs);
        let mut bodies = Vec::with_capacity(heredocs.len());

        for heredoc in &heredocs {
            let body = self.read_heredoc_body(heredoc)?;
            bodies.push(body);
        }

        let newline_span = self.cursor.span_from(start, start_ln, start_col);
        for body in bodies {
            self.queued
                .push_back(Token::new(TokenKind::HereDocBody(body), newline_span));
        }
        self.queued
            .push_back(Token::new(TokenKind::Newline, newline_span));
        Ok(())
    }

    fn read_heredoc_body(&mut self, heredoc: &PendingHereDoc) -> Result<String, ShellError> {
        let mut body = String::new();

        loop {
            if self.cursor.peek().is_none() {
                // bash: EOF before the terminator is a warning, not an error —
                // the body runs to end of file and lexing continues.
                break;
            }

            let mut line = String::new();
            while let Some(c) = self.cursor.peek() {
                if c == '\n' {
                    break;
                }
                line.push(c);
                self.cursor.advance();
            }

            let cmp = if heredoc.strip_tabs {
                line.trim_start_matches('\t')
            } else {
                line.as_str()
            };

            if cmp == heredoc.delimiter {
                if self.cursor.peek() == Some('\n') {
                    self.cursor.advance();
                } else if self.cursor.peek().is_none() {
                    // EOF immediately after terminator is acceptable.
                }
                break;
            }

            if heredoc.strip_tabs {
                body.push_str(line.trim_start_matches('\t'));
            } else {
                body.push_str(&line);
            }

            match self.cursor.peek() {
                Some('\n') => {
                    self.cursor.advance();
                    body.push('\n');
                }
                None => break, // EOF: keep the accumulated body (bash warning case)
                _ => unreachable!(),
            }
        }

        Ok(body)
    }
}

fn read_cmd_sub_body(cursor: &mut Cursor, s: &mut String) {
    let mut depth = 1usize;
    let mut in_case = 0usize; // tracks case..esac nesting so ) in patterns isn't mistaken for group closer
    let mut word_buf = String::new();

    loop {
        match cursor.advance() {
            None => break,
            Some('(') => {
                flush_word(&word_buf, &mut in_case);
                word_buf.clear();
                depth += 1;
                s.push('(');
            }
            Some(')') => {
                flush_word(&word_buf, &mut in_case);
                word_buf.clear();
                // A ) at the outermost depth while inside a case block is a
                // case-pattern terminator, not the closing ) of the $().
                if in_case > 0 && depth == 1 {
                    s.push(')');
                } else {
                    depth -= 1;
                    s.push(')');
                    if depth == 0 {
                        break;
                    }
                }
            }
            Some(c) if c.is_alphanumeric() || c == '_' => {
                word_buf.push(c);
                s.push(c);
            }
            Some(c) => {
                flush_word(&word_buf, &mut in_case);
                word_buf.clear();
                s.push(c);
            }
        }
    }
}

#[inline]
fn flush_word(word: &str, in_case: &mut usize) {
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

fn read_backtick_body(cursor: &mut Cursor, s: &mut String) {
    loop {
        match cursor.advance() {
            None | Some('`') => {
                s.push('`');
                break;
            }
            Some(c) => s.push(c),
        }
    }
}

/// ANSI-C quoting body: `$'...'` — decode escapes into literal chars.
fn read_ansi_c_body(cursor: &mut Cursor, s: &mut String) {
    loop {
        match cursor.advance() {
            None | Some('\'') => break,
            Some('\\') => match cursor.advance() {
                Some('a') => s.push('\u{07}'),
                Some('b') => s.push('\u{08}'),
                Some('e') | Some('E') => s.push('\u{1B}'),
                Some('f') => s.push('\u{0C}'),
                Some('n') => s.push('\n'),
                Some('r') => s.push('\r'),
                Some('t') => s.push('\t'),
                Some('v') => s.push('\u{0B}'),
                Some('\\') => s.push('\\'),
                Some('\'') => s.push('\''),
                Some('"') => s.push('"'),
                Some('?') => s.push('?'),
                Some(c) if c.is_ascii_digit() => {
                    // \0NNN octal (also \NNN after the leading digit)
                    let mut digits = String::new();
                    digits.push(c);
                    while digits.len() < 3 {
                        match cursor.peek() {
                            Some(d) if d.is_ascii_digit() => {
                                digits.push(d);
                                cursor.advance();
                            }
                            _ => break,
                        }
                    }
                    let code = u32::from_str_radix(&digits, 8).unwrap_or(0);
                    if let Some(ch) = char::from_u32(code & 0x1FFFFF) {
                        s.push(ch);
                    }
                }
                Some('x') => {
                    let mut digits = String::new();
                    while digits.len() < 2 {
                        match cursor.peek() {
                            Some(d) if d.is_ascii_hexdigit() => {
                                digits.push(d);
                                cursor.advance();
                            }
                            _ => break,
                        }
                    }
                    let code = u32::from_str_radix(&digits, 16).unwrap_or(0);
                    if let Some(ch) = char::from_u32(code) {
                        s.push(ch);
                    }
                }
                // \cX: control char — X & 0x1F (e.g. \cA → 0x01, \c[ → ESC,
                // \cz → 0x1A; XOR-0x40 breaks lowercase inputs).
                Some('c') => match cursor.advance() {
                    Some(c) => {
                        let code = (c as u32) & 0x1F;
                        if let Some(ch) = char::from_u32(code) {
                            s.push(ch);
                        }
                    }
                    None => s.push('\\'),
                },
                Some(c) => {
                    s.push('\\');
                    s.push(c);
                }
                None => s.push('\\'),
            },
            Some(c) => s.push(c),
        }
    }
}
