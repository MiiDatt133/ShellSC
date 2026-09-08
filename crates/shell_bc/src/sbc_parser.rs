use crate::bytecode::{RedirEntry, RedirKind, RedirTarget};
use shell_ast::ShellError;

#[derive(Debug, Clone)]
pub struct SbcInstr {
    pub mnemonic: String,
    pub args: Vec<SbcArg>,
}

#[derive(Debug, Clone)]
pub enum SbcArg {
    Str(String),
    Uint(u32),
}

pub struct SbcParser<'a> {
    input: &'a str,
}

impl<'a> SbcParser<'a> {
    pub fn new(input: &'a str) -> Self {
        Self { input }
    }

    pub fn parse(&self) -> Result<Vec<SbcInstr>, ShellError> {
        let mut instrs = vec![];
        for (lineno, raw) in self.input.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
                continue;
            }
            instrs.push(self.parse_line(line, lineno + 1)?);
        }
        Ok(instrs)
    }

    fn parse_line(&self, line: &str, lineno: usize) -> Result<SbcInstr, ShellError> {
        let mut chars = line.chars().peekable();
        let mnemonic = Self::read_token(&mut chars);
        if mnemonic.is_empty() {
            return Err(ShellError::BytecodeError(format!(
                "line {}: empty mnemonic",
                lineno
            )));
        }
        let rest: String = chars.collect();
        let rest = rest.trim();
        let args = if rest.is_empty() {
            vec![]
        } else {
            Self::parse_args(rest, lineno)?
        };
        Ok(SbcInstr { mnemonic, args })
    }

    fn read_token(chars: &mut std::iter::Peekable<std::str::Chars>) -> String {
        let mut s = String::new();
        while let Some(&c) = chars.peek() {
            if c.is_whitespace() {
                break;
            }
            s.push(c);
            chars.next();
        }
        while let Some(&c) = chars.peek() {
            if !c.is_whitespace() {
                break;
            }
            chars.next();
        }
        s
    }

    fn parse_args(rest: &str, lineno: usize) -> Result<Vec<SbcArg>, ShellError> {
        let mut args = vec![];
        let mut pos = 0;
        let bytes = rest.as_bytes();

        while pos < bytes.len() {
            while pos < bytes.len() && (bytes[pos] as char).is_whitespace() {
                pos += 1;
            }
            if pos >= bytes.len() {
                break;
            }

            if bytes[pos] == b'"' {
                let (s, consumed) = Self::parse_quoted(&rest[pos..], lineno)?;
                args.push(SbcArg::Str(s));
                pos += consumed;
            } else {
                let start = pos;
                while pos < bytes.len() && !(bytes[pos] as char).is_whitespace() {
                    pos += 1;
                }
                let tok = &rest[start..pos];
                if let Ok(n) = tok.parse::<u32>() {
                    args.push(SbcArg::Uint(n));
                } else {
                    args.push(SbcArg::Str(tok.to_string()));
                }
            }
        }
        Ok(args)
    }

    /// Parse a double-quoted string from `input` (which must start with `"`).
    /// Returns the unescaped string and the number of **bytes** consumed.
    ///
    /// Uses `char_indices` so multi-byte UTF-8 sequences (e.g. ✓ U+2713) are
    /// decoded correctly.  The old byte-by-byte approach produced garbled output
    /// for non-ASCII characters (E2 9C 93 became 3 separate Latin-1 chars).
    fn parse_quoted(input: &str, lineno: usize) -> Result<(String, usize), ShellError> {
        let mut iter = input.char_indices();
        let (_, first) = iter.next().expect("non-empty");
        debug_assert_eq!(first, '"');

        let mut s = String::new();
        let end_byte; // set exactly once, when we find the closing "

        loop {
            match iter.next() {
                None => {
                    return Err(ShellError::BytecodeError(format!(
                        "line {}: unterminated string",
                        lineno
                    )))
                }
                Some((i, '"')) => {
                    end_byte = i + 1;
                    break;
                }
                Some((_, '\\')) => match iter.next() {
                    None => {
                        return Err(ShellError::BytecodeError(format!(
                            "line {}: trailing backslash",
                            lineno
                        )))
                    }
                    Some((_, c)) => match c {
                        '"' => s.push('"'),
                        '\\' => s.push('\\'),
                        'n' => s.push('\n'),
                        'r' => s.push('\r'),
                        't' => s.push('\t'),
                        c => {
                            s.push('\\');
                            s.push(c);
                        }
                    },
                },
                Some((_, c)) => {
                    s.push(c);
                }
            }
        }
        Ok((s, end_byte))
    }
}

/// Parse Redirect args: [Str(kind), Uint(fd), Str("File"|"Fd"|"HereDoc"), Str(path)|Uint(n)]
pub fn parse_redir_args(args: &[SbcArg], lineno: usize) -> Result<RedirEntry, ShellError> {
    if args.len() < 4 {
        return Err(ShellError::BytecodeError(format!(
            "line {}: Redirect needs 4 args",
            lineno
        )));
    }
    let kind = match &args[0] {
        SbcArg::Str(s) => match s.as_str() {
            "Out" => RedirKind::Out,
            "Append" => RedirKind::Append,
            "In" => RedirKind::In,
            "OutFd" => RedirKind::OutFd,
            "InFd" => RedirKind::InFd,
            "HereDoc" => RedirKind::HereDoc,
            "HereDocLit" => RedirKind::HereDocLit,
            other => {
                return Err(ShellError::BytecodeError(format!(
                    "line {}: unknown redir kind '{}'",
                    lineno, other
                )))
            }
        },
        _ => {
            return Err(ShellError::BytecodeError(format!(
                "line {}: bad Redirect kind",
                lineno
            )))
        }
    };
    let fd = match &args[1] {
        SbcArg::Uint(n) => *n,
        _ => {
            return Err(ShellError::BytecodeError(format!(
                "line {}: bad Redirect fd",
                lineno
            )))
        }
    };
    let target_type = match &args[2] {
        SbcArg::Str(s) => s.clone(),
        _ => {
            return Err(ShellError::BytecodeError(format!(
                "line {}: bad Redirect target type",
                lineno
            )))
        }
    };
    let target = match target_type.as_str() {
        "File" => match &args[3] {
            SbcArg::Str(p) => RedirTarget::FilePath(p.clone()),
            SbcArg::Uint(n) => RedirTarget::FilePath(n.to_string()),
        },
        "Fd" => match &args[3] {
            SbcArg::Uint(n) => RedirTarget::Fd(*n),
            SbcArg::Str(s) => RedirTarget::Fd(s.parse::<u32>().map_err(|_| {
                ShellError::BytecodeError(format!("line {}: bad Fd number", lineno))
            })?),
        },
        "HereDoc" => match &args[3] {
            SbcArg::Str(s) => RedirTarget::HereDoc(s.clone()),
            SbcArg::Uint(n) => RedirTarget::HereDoc(n.to_string()),
        },
        other => {
            return Err(ShellError::BytecodeError(format!(
                "line {}: unknown redirect target '{}'",
                lineno, other
            )))
        }
    };
    Ok(RedirEntry { kind, fd, target })
}
