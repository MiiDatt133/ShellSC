// === FILE: crates/shell_lex/src/lib.rs ===
pub mod cursor;
pub mod lexer;
pub mod token;

pub use lexer::Lexer;
pub use token::{Token, TokenKind};
// === END FILE ===
