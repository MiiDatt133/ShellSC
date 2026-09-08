// === FILE: crates/shell_parse/src/precedence.rs ===
use shell_lex::token::TokenKind;

/// Boolean-chain precedence level.
/// Higher = binds tighter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Prec {
    /// Lowest — used as initial "minimum" when starting a parse.
    None = 0,
    /// `||`
    Or = 1,
    /// `&&`
    And = 2,
}

impl Prec {
    /// Return the precedence of the given operator token, if it is one.
    pub fn of(kind: &TokenKind) -> Option<Prec> {
        match kind {
            TokenKind::Or => Some(Prec::Or),
            TokenKind::And => Some(Prec::And),
            _ => None,
        }
    }

    /// One level higher than `self` (used for left-associative parsing).
    pub fn next(self) -> Prec {
        match self {
            Prec::None => Prec::Or,
            Prec::Or => Prec::And,
            Prec::And => Prec::And, // already max
        }
    }
}
// === END FILE ===
