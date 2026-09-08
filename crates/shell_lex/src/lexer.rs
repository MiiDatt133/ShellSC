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
                return Err(ShellError::lex(
                    Span::new(
                        self.cursor.pos(),
                        self.cursor.pos(),
                        self.cursor.line,
                        self.cursor.col,
                    ),
                    format!("unterminated heredoc for delimiter '{}'", heredoc.delimiter),
                ));
            }

            let line_start = self.cursor.pos();
            let line_ln = self.cursor.line;
            let line_col = self.cursor.col;
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
                None => {
                    return Err(ShellError::lex(
                        Span::new(line_start, self.cursor.pos(), line_ln, line_col),
                        format!("unterminated heredoc for delimiter '{}'", heredoc.delimiter),
                    ));
                }
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
