//! The LogQL lexer.
//!
//! Hand-written, over a slice of characters so every token carries the
//! character position the parser's errors point at. It follows the rules of
//! Loki's `pkg/logql/syntax/lex.go` where they matter for queries copied from
//! Grafana:
//!
//! - Strings are `"..."` with Go-style escapes, or `` `...` `` raw. A double
//!   quoted `"\d"` is an error, as in Go: write `` `\d` `` or `"\\d"`.
//! - `#` starts a comment that runs to the end of the line.
//! - A number followed by letters is one token: a duration (`500ms`, `1h30m`),
//!   or a byte size (`10MB`, which loctel does not support and says so).
//! - Words are letters, digits and `_`. Which of them are keywords (`json`,
//!   `and`, `line_format`) depends on where they stand, so the parser decides,
//!   and it compares them without regard to case, as Loki does.
//!
//! Lexing never fails as a whole: a bad token becomes a [`Tok::Invalid`]
//! carrying its error, and the parser reports it if it gets that far, so an
//! earlier syntax error in the query wins over a later bad character.

use loctel_model::loki_label_name;

use crate::{QueryError, parse_duration};

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Tok {
    LBrace,
    RBrace,
    LParen,
    RParen,
    Comma,
    /// `|`
    Pipe,
    /// `|=`
    PipeEq,
    /// `|~`
    PipeRe,
    /// `|>`: Loki's pattern line filter.
    PipePattern,
    /// `=`
    Eq,
    /// `==`
    EqEq,
    /// `!=`
    Neq,
    /// `=~`
    Re,
    /// `!~`
    Nre,
    /// `!>`: Loki's negated pattern line filter.
    NotPattern,
    Gt,
    Gte,
    Lt,
    Lte,
    Plus,
    Minus,
    Str(String),
    Number(f64),
    /// Nanoseconds.
    Duration(u64),
    /// A word: a label name, or a keyword depending on where it stands.
    Ident(String),
    /// A token that could not be lexed.
    Invalid(QueryError),
    Eof,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Token {
    pub tok: Tok,
    /// Character index of the first character.
    pub start: usize,
    /// Character index just past the last character.
    pub end: usize,
}

/// Byte-size suffixes Loki accepts in a literal (`10MB`), compared in lower
/// case. loctel does not support them in label filters yet.
const BYTE_UNITS: &[&str] = &[
    "b", "kb", "kib", "mb", "mib", "gb", "gib", "tb", "tib", "pb", "pib",
];

/// Lexes the whole query. The last token is always [`Tok::Eof`], positioned at
/// the end of the text.
pub(crate) fn lex(chars: &[char]) -> Vec<Token> {
    let mut lexer = Lexer { chars, pos: 0 };
    let mut tokens = Vec::new();
    loop {
        lexer.skip_trivia();
        let start = lexer.pos;
        if start >= chars.len() {
            tokens.push(Token {
                tok: Tok::Eof,
                start,
                end: start,
            });
            return tokens;
        }
        match lexer.next_token() {
            Ok(tok) => tokens.push(Token {
                tok,
                start,
                end: lexer.pos,
            }),
            Err(err) => {
                let at = err.position;
                tokens.push(Token {
                    tok: Tok::Invalid(err),
                    start: at,
                    end: lexer.pos.max(at),
                });
                tokens.push(Token {
                    tok: Tok::Eof,
                    start: chars.len(),
                    end: chars.len(),
                });
                return tokens;
            }
        }
    }
}

struct Lexer<'a> {
    chars: &'a [char],
    pos: usize,
}

fn is_word_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Lexer<'_> {
    fn peek(&self) -> Option<char> {
        self.chars.get(self.pos).copied()
    }

    fn peek_at(&self, offset: usize) -> Option<char> {
        self.chars.get(self.pos + offset).copied()
    }

    /// Skips whitespace and `#` comments.
    fn skip_trivia(&mut self) {
        loop {
            match self.peek() {
                Some(c) if c.is_whitespace() => self.pos += 1,
                Some('#') => {
                    while self.peek().is_some_and(|c| c != '\n') {
                        self.pos += 1;
                    }
                }
                _ => return,
            }
        }
    }

    /// Consumes `n` characters and gives back `tok`.
    fn take(&mut self, n: usize, tok: Tok) -> Result<Tok, QueryError> {
        self.pos += n;
        Ok(tok)
    }

    fn next_token(&mut self) -> Result<Tok, QueryError> {
        let start = self.pos;
        let c = self.chars[start];
        match c {
            '{' => self.take(1, Tok::LBrace),
            '}' => self.take(1, Tok::RBrace),
            '(' => self.take(1, Tok::LParen),
            ')' => self.take(1, Tok::RParen),
            ',' => self.take(1, Tok::Comma),
            '|' => match self.peek_at(1) {
                Some('=') => self.take(2, Tok::PipeEq),
                Some('~') => self.take(2, Tok::PipeRe),
                Some('>') => self.take(2, Tok::PipePattern),
                _ => self.take(1, Tok::Pipe),
            },
            '=' => match self.peek_at(1) {
                Some('=') => self.take(2, Tok::EqEq),
                Some('~') => self.take(2, Tok::Re),
                _ => self.take(1, Tok::Eq),
            },
            '!' => match self.peek_at(1) {
                Some('=') => self.take(2, Tok::Neq),
                Some('~') => self.take(2, Tok::Nre),
                Some('>') => self.take(2, Tok::NotPattern),
                _ => Err(QueryError::new(start, "unexpected character `!`")),
            },
            '>' => match self.peek_at(1) {
                Some('=') => self.take(2, Tok::Gte),
                _ => self.take(1, Tok::Gt),
            },
            '<' => match self.peek_at(1) {
                Some('=') => self.take(2, Tok::Lte),
                _ => self.take(1, Tok::Lt),
            },
            '+' => self.take(1, Tok::Plus),
            '-' => self.take(1, Tok::Minus),
            '"' => self.string(),
            '`' => self.raw_string(),
            '0'..='9' => self.number(),
            // `.5` is a number; any other `.` is not part of LogQL.
            '.' if self.peek_at(1).is_some_and(|d| d.is_ascii_digit()) => self.number(),
            c if is_word_start(c) => self.word(),
            c => {
                let shown = if c.is_control() {
                    c.escape_default().to_string()
                } else {
                    c.to_string()
                };
                Err(QueryError::new(
                    start,
                    format!("unexpected character `{shown}`"),
                ))
            }
        }
    }

    /// A word. A label name is letters, digits and `_` only, so a `.` or `-`
    /// glued to the end of one (`deployment.environment`, `http-method`) is a
    /// mistake that comes up all the time with OpenTelemetry attribute names:
    /// the error says how loctel spells that label.
    fn word(&mut self) -> Result<Tok, QueryError> {
        let start = self.pos;
        while self.peek().is_some_and(is_word_char) {
            self.pos += 1;
        }
        if matches!(self.peek(), Some('.' | '-')) && self.peek_at(1).is_some_and(is_word_char) {
            while self
                .peek()
                .is_some_and(|c| is_word_char(c) || c == '.' || c == '-')
            {
                self.pos += 1;
            }
            let written: String = self.chars[start..self.pos].iter().collect();
            return Err(QueryError::new(
                start,
                format!(
                    "invalid label name `{written}`: a label name is letters, digits and `_` only (loctel writes it `{}`)",
                    loki_label_name(&written)
                ),
            ));
        }
        let word: String = self.chars[start..self.pos].iter().collect();
        Ok(Tok::Ident(word))
    }

    /// `"text"` with Go-style escapes: `\a \b \f \n \r \t \v \\ \"`, `\ooo`
    /// and `\xHH` (ASCII only, so the text stays UTF-8), `\uHHHH`, `\UHHHHHHHH`.
    fn string(&mut self) -> Result<Tok, QueryError> {
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(QueryError::new(start, "unterminated string")),
                Some('"') => {
                    self.pos += 1;
                    return Ok(Tok::Str(out));
                }
                Some('\\') => out.push(self.escape(start)?),
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// The escape sequence at the cursor (a backslash). `string_start` is where
    /// the string began, for the unterminated-string error.
    fn escape(&mut self, string_start: usize) -> Result<char, QueryError> {
        let at = self.pos;
        let Some(kind) = self.peek_at(1) else {
            return Err(QueryError::new(string_start, "unterminated string"));
        };
        let simple = match kind {
            'a' => Some('\u{7}'),
            'b' => Some('\u{8}'),
            'f' => Some('\u{c}'),
            'n' => Some('\n'),
            'r' => Some('\r'),
            't' => Some('\t'),
            'v' => Some('\u{b}'),
            '\\' => Some('\\'),
            '"' => Some('"'),
            _ => None,
        };
        if let Some(c) = simple {
            self.pos += 2;
            return Ok(c);
        }
        // `\xHH`, `\uHHHH`, `\UHHHHHHHH` and `\ooo`: where the digits start,
        // how many there are and their base.
        let (skip, digits, radix) = match kind {
            'x' => (2, 2, 16),
            'u' => (2, 4, 16),
            'U' => (2, 8, 16),
            '0'..='7' => (1, 3, 8),
            _ => {
                return Err(QueryError::new(
                    at,
                    format!("invalid escape sequence `\\{kind}`"),
                ));
            }
        };
        let end = (at + skip + digits).min(self.chars.len());
        let written: String = self.chars[at..end].iter().collect();
        let invalid = || QueryError::new(at, format!("invalid escape sequence `{written}`"));
        let digits: String = self
            .chars
            .get(at + skip..end)
            .map(|d| d.iter().collect::<String>())
            .filter(|text| {
                text.chars().count() == digits && text.chars().all(|c| c.is_digit(radix))
            })
            .ok_or_else(invalid)?;
        let value = u32::from_str_radix(&digits, radix).map_err(|_| invalid())?;
        // `\x` and octal escapes are bytes in Go; only the ASCII ones are the
        // same character here.
        if matches!(kind, 'x' | '0'..='7') && value > 0x7f {
            return Err(invalid());
        }
        let c = char::from_u32(value).ok_or_else(invalid)?;
        self.pos = end;
        Ok(c)
    }

    /// `` `text` ``: no escapes at all, which keeps regexes readable.
    fn raw_string(&mut self) -> Result<Tok, QueryError> {
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(QueryError::new(start, "unterminated string")),
                Some('`') => {
                    self.pos += 1;
                    return Ok(Tok::Str(out));
                }
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// A number, a duration or a byte size. The lexer takes the whole run of
    /// digits, dots and letters (so `1h30m` and `500ms` stay in one piece, and
    /// so does `1e-3`), then decides what it is.
    fn number(&mut self) -> Result<Tok, QueryError> {
        let start = self.pos;
        while let Some(c) = self.peek() {
            if c.is_ascii_digit() || c == '.' || c.is_alphabetic() {
                self.pos += 1;
            } else if matches!(c, '+' | '-')
                && self.pos > start
                && matches!(self.chars[self.pos - 1], 'e' | 'E')
                && self.chars[start..self.pos - 1]
                    .iter()
                    .all(|m| m.is_ascii_digit() || *m == '.')
                && self.pos - 1 > start
                && self.peek_at(1).is_some_and(|d| d.is_ascii_digit())
            {
                // The sign of an exponent: `1e-3`.
                self.pos += 1;
            } else {
                break;
            }
        }
        let text: String = self.chars[start..self.pos].iter().collect();

        if text.chars().all(|c| c.is_ascii_digit())
            && let Ok(n) = text.parse::<f64>()
            && n.is_finite()
        {
            return Ok(Tok::Number(n));
        }
        if let Some(nanos) = parse_duration(&text) {
            return Ok(Tok::Duration(nanos));
        }
        if text
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'))
        {
            return match text.parse::<f64>() {
                Ok(n) if n.is_finite() => Ok(Tok::Number(n)),
                _ => Err(QueryError::new(start, format!("invalid number `{text}`"))),
            };
        }
        // `10MB`: a number and a byte unit.
        let unit_at = text.find(|c: char| c.is_alphabetic()).unwrap_or(text.len());
        let (amount, unit) = text.split_at(unit_at);
        if amount.parse::<f64>().is_ok() && BYTE_UNITS.contains(&unit.to_lowercase().as_str()) {
            return Err(QueryError::unsupported(start, "a byte size"));
        }
        let message = if text.chars().any(char::is_alphabetic) {
            format!("invalid duration `{text}` (units are ns, us, µs, ms, s, m, h)")
        } else {
            format!("invalid number `{text}`")
        };
        Err(QueryError::new(start, message))
    }
}
