// === FILE: crates/shell_parse/src/consume.rs ===
use shell_ast::{ShellError, Span};
use shell_lex::token::{Token, TokenKind};

/// Stateful token stream with lookahead and consume helpers.
pub struct TokenStream {
    tokens: Vec<Token>,
    pos: usize,
}

impl TokenStream {
    pub fn new(tokens: Vec<Token>) -> Self {
        Self { tokens, pos: 0 }
    }

    // ──────────────────────────────────────────
    // Peek / lookahead
    // ──────────────────────────────────────────

    /// Peek at current token without consuming.
    pub fn peek(&self) -> &Token {
        self.tokens
            .get(self.pos)
            .unwrap_or_else(|| self.eof_token())
    }

    /// Peek at token N positions ahead (0 = current).
    pub fn peek_at(&self, offset: usize) -> &Token {
        self.tokens
            .get(self.pos + offset)
            .unwrap_or_else(|| self.eof_token())
    }

    pub fn prev_token(&self) -> Option<&Token> {
        if self.pos > 0 {
            self.tokens.get(self.pos - 1)
        } else {
            None
        }
    }

    pub fn peek_kind(&self) -> &TokenKind {
        &self.peek().kind
    }

    // ──────────────────────────────────────────
    // Advance
    // ──────────────────────────────────────────

    /// Consume and return current token.
    pub fn advance(&mut self) -> &Token {
        let tok = &self.tokens[self.pos.min(self.tokens.len() - 1)];
        if self.pos < self.tokens.len() {
            self.pos += 1;
        }
        tok
    }

    /// Consume current token; return its span.
    pub fn advance_span(&mut self) -> Span {
        self.advance().span
    }

    // ──────────────────────────────────────────
    // Conditional consume
    // ──────────────────────────────────────────

    /// Consume if current token matches kind, else return None.
    pub fn eat(&mut self, kind: &TokenKind) -> Option<Span> {
        if self.peek_kind() == kind {
            Some(self.advance_span())
        } else {
            None
        }
    }

    /// Consume newlines and semicolons (statement terminators).
    pub fn skip_terminators(&mut self) {
        loop {
            match self.peek_kind() {
                TokenKind::Newline | TokenKind::Semi => {
                    self.advance();
                }
                _ => break,
            }
        }
    }

    /// Consume newlines only (not semicolons).
    pub fn skip_newlines(&mut self) {
        loop {
            if matches!(self.peek_kind(), TokenKind::Newline) {
                self.advance();
            } else {
                break;
            }
        }
    }

    // ──────────────────────────────────────────
    // Require
    // ──────────────────────────────────────────

    /// Consume expected token kind or return parse error.
    pub fn expect(&mut self, kind: &TokenKind) -> Result<Span, ShellError> {
        if self.peek_kind() == kind {
            Ok(self.advance_span())
        } else {
            let tok = self.peek();
            Err(ShellError::unexpected_token(
                format!("expected '{}', got '{}'", kind, tok.kind),
                tok.span,
            ))
        }
    }

    // ──────────────────────────────────────────
    // State
    // ──────────────────────────────────────────

    pub fn is_eof(&self) -> bool {
        matches!(self.peek_kind(), TokenKind::Eof)
    }

    pub fn current_span(&self) -> Span {
        self.peek().span
    }

    // ──────────────────────────────────────────
    // Private
    // ──────────────────────────────────────────

    fn eof_token(&self) -> &Token {
        // Safe: tokenize() always appends Eof.
        self.tokens.last().expect("token list must end with Eof")
    }
}
// === END FILE ===
