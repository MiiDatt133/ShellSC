use shell_ast::Span;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    pub span: Span,
}

impl Token {
    pub fn new(kind: TokenKind, span: Span) -> Self {
        Self { kind, span }
    }

    pub fn is_word(&self) -> bool {
        matches!(
            &self.kind,
            TokenKind::Word(_) | TokenKind::SingleQuoted(_) | TokenKind::DoubleQuoted(_)
        )
    }

    pub fn is_newline_or_semi(&self) -> bool {
        matches!(self.kind, TokenKind::Newline | TokenKind::Semi)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    Word(String),
    SingleQuoted(String),
    DoubleQuoted(String),

    // ── Operators ──────────────────────────────
    /// |
    Pipe,
    /// &&
    And,
    /// ||
    Or,
    /// ;
    Semi,
    /// &
    Ampersand,
    /// (
    LParen,
    /// )
    RParen,
    /// {
    LBrace,
    /// }
    RBrace,

    /// >
    RedirOut,
    /// >>
    RedirAppend,
    /// <
    RedirIn,
    /// >&
    RedirOutFd,
    /// <&
    RedirInFd,
    /// <<
    HereDoc,
    /// <<-
    HereDocStrip,
    /// <<<
    HereString,
    /// synthetic heredoc body token emitted before the newline terminator
    HereDocBody(String),

    /// if
    If,
    /// then
    Then,
    /// else
    Else,
    /// elif
    Elif,
    /// fi
    Fi,
    /// while
    While,
    /// until
    Until,
    /// do
    Do,
    /// done
    Done,
    /// for
    For,
    /// in
    In,
    /// case
    Case,
    /// esac
    Esac,
    /// function
    Function,

    /// \n
    Newline,
    /// End of input.
    Eof,
}

impl TokenKind {
    pub fn is_word_start(&self) -> bool {
        matches!(
            self,
            TokenKind::Word(_) | TokenKind::SingleQuoted(_) | TokenKind::DoubleQuoted(_)
        )
    }

    pub fn keyword_or_word(s: String) -> TokenKind {
        match s.as_str() {
            "if" => TokenKind::If,
            "then" => TokenKind::Then,
            "else" => TokenKind::Else,
            "elif" => TokenKind::Elif,
            "fi" => TokenKind::Fi,
            "while" => TokenKind::While,
            "until" => TokenKind::Until,
            "do" => TokenKind::Do,
            "done" => TokenKind::Done,
            "for" => TokenKind::For,
            "in" => TokenKind::In,
            "case" => TokenKind::Case,
            "esac" => TokenKind::Esac,
            "function" => TokenKind::Function,
            _ => TokenKind::Word(s),
        }
    }

    /// Reserved words are valid as arguments outside command position.
    /// Returns the keyword's string form if this is a keyword token.
    pub fn as_keyword_str(&self) -> Option<&'static str> {
        match self {
            TokenKind::If => Some("if"),
            TokenKind::Then => Some("then"),
            TokenKind::Else => Some("else"),
            TokenKind::Elif => Some("elif"),
            TokenKind::Fi => Some("fi"),
            TokenKind::While => Some("while"),
            TokenKind::Until => Some("until"),
            TokenKind::Do => Some("do"),
            TokenKind::Done => Some("done"),
            TokenKind::For => Some("for"),
            TokenKind::In => Some("in"),
            TokenKind::Case => Some("case"),
            TokenKind::Esac => Some("esac"),
            TokenKind::Function => Some("function"),
            _ => None,
        }
    }
}

impl std::fmt::Display for TokenKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TokenKind::Word(s) => write!(f, "{}", s),
            TokenKind::SingleQuoted(s) => write!(f, "'{}'", s),
            TokenKind::DoubleQuoted(s) => write!(f, "\"{}\"", s),
            TokenKind::Pipe => write!(f, "|"),
            TokenKind::And => write!(f, "&&"),
            TokenKind::Or => write!(f, "||"),
            TokenKind::Semi => write!(f, ";"),
            TokenKind::Ampersand => write!(f, "&"),
            TokenKind::LParen => write!(f, "("),
            TokenKind::RParen => write!(f, ")"),
            TokenKind::LBrace => write!(f, "{{"),
            TokenKind::RBrace => write!(f, "}}"),
            TokenKind::RedirOut => write!(f, ">"),
            TokenKind::RedirAppend => write!(f, ">>"),
            TokenKind::RedirIn => write!(f, "<"),
            TokenKind::RedirOutFd => write!(f, ">&"),
            TokenKind::RedirInFd => write!(f, "<&"),
            TokenKind::HereDoc => write!(f, "<<"),
            TokenKind::HereDocStrip => write!(f, "<<-"),
            TokenKind::HereString => write!(f, "<<<"),
            TokenKind::If => write!(f, "if"),
            TokenKind::Then => write!(f, "then"),
            TokenKind::Else => write!(f, "else"),
            TokenKind::Elif => write!(f, "elif"),
            TokenKind::Fi => write!(f, "fi"),
            TokenKind::While => write!(f, "while"),
            TokenKind::Until => write!(f, "until"),
            TokenKind::Do => write!(f, "do"),
            TokenKind::Done => write!(f, "done"),
            TokenKind::For => write!(f, "for"),
            TokenKind::In => write!(f, "in"),
            TokenKind::Case => write!(f, "case"),
            TokenKind::Esac => write!(f, "esac"),
            TokenKind::Function => write!(f, "function"),
            TokenKind::HereDocBody(_) => write!(f, "<HEREDOC_BODY>"),
            TokenKind::Newline => write!(f, "\\n"),
            TokenKind::Eof => write!(f, "<EOF>"),
        }
    }
}
