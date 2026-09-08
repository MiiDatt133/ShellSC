use crate::span::Span;
use thiserror::Error;

#[derive(Debug, Error, Clone)]
pub enum ShellError {
    #[error("lex error at {span}: {msg}")]
    LexError { span: Span, msg: String },

    #[error("parse error at {span}: {msg}")]
    ParseError { span: Span, msg: String },

    #[error("unexpected token '{token}' at {span}")]
    UnexpectedToken { token: String, span: Span },

    #[error("unexpected end of input")]
    UnexpectedEof,

    #[error("not implemented: {0}")]
    NotImplemented(String),

    #[error("lower error: {0}")]
    LowerError(String),

    #[error("bytecode error: {0}")]
    BytecodeError(String),

    #[error("vm error: {0}")]
    VmError(String),

    #[error("io error: {0}")]
    IoError(String),
}

impl ShellError {
    pub fn lex(span: Span, msg: impl Into<String>) -> Self {
        Self::LexError {
            span,
            msg: msg.into(),
        }
    }

    pub fn parse(span: Span, msg: impl Into<String>) -> Self {
        Self::ParseError {
            span,
            msg: msg.into(),
        }
    }

    pub fn unexpected_token(token: impl Into<String>, span: Span) -> Self {
        Self::UnexpectedToken {
            token: token.into(),
            span,
        }
    }

    pub fn not_implemented(feature: impl Into<String>) -> Self {
        Self::NotImplemented(feature.into())
    }
}
