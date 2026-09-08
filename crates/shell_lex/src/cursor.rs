use shell_ast::Span;

pub struct Cursor<'a> {
    src: &'a str,
    chars: std::iter::Peekable<std::str::CharIndices<'a>>,
    pos: usize,
    pub line: u32,
    pub col: u32,
}

impl<'a> Cursor<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            chars: src.char_indices().peekable(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    pub fn peek(&mut self) -> Option<char> {
        self.chars.peek().map(|&(_, c)| c)
    }

    pub fn peek2(&self) -> Option<char> {
        let mut it = self.src[self.pos..].chars();
        it.next();
        it.next()
    }

    pub fn advance(&mut self) -> Option<char> {
        match self.chars.next() {
            None => None,
            Some((byte_pos, c)) => {
                self.pos = byte_pos + c.len_utf8();
                if c == '\n' {
                    self.line += 1;
                    self.col = 1;
                } else {
                    self.col += 1;
                }
                Some(c)
            }
        }
    }

    pub fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.advance();
            true
        } else {
            false
        }
    }

    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn span_from(&self, start: usize, start_line: u32, start_col: u32) -> Span {
        Span::new(start, self.pos, start_line, start_col)
    }

    pub fn is_eof(&mut self) -> bool {
        self.peek().is_none()
    }
}
