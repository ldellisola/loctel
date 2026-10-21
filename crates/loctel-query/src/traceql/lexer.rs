//! The TraceQL lexer.
//!
//! Hand-written, over a slice of characters so every token carries the
//! character position the parser's errors point at. It follows the rules of
//! Tempo's `pkg/traceql/lexer.go` where they matter for queries copied from
//! Grafana:
//!
//! - After `.`, `span.` or `resource.` the lexer reads an *attribute name*,
//!   which runs until whitespace or one of `{}()=~!<>&|^,`. That is why
//!   `.http.status_code` and `span.http-route` are single names, and why
//!   `{.a="x"||.b="y"}` needs no spaces. A name may hold `"quoted"` parts
//!   (`span."http status"`) with `\"` and `\\` escapes.
//! - A number followed by duration units is one duration token (`1h30m`).
//! - `span:`, `trace:`, `event:`, `link:` and `instrumentation:` start a
//!   scoped intrinsic (`span:name`).
//!
//! Lexing never fails as a whole: a bad token becomes a [`Tok::Invalid`]
//! carrying its error, and the parser reports it if it gets that far, so an
//! earlier syntax error in the query wins over a later bad character.

use crate::{QueryError, parse_duration};

/// What an attribute token is scoped to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScopeKind {
    Span,
    Resource,
    /// A bare `.name`.
    Any,
    // Scopes Tempo has and loctel does not: recognised so the parser can say
    // so, instead of failing somewhere confusing.
    Parent,
    Event,
    Link,
    Instrumentation,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Tok {
    LBrace,
    RBrace,
    LParen,
    RParen,
    Comma,
    Pipe,
    /// `&&`
    AndAnd,
    /// `||`
    OrOr,
    /// `!`
    Not,
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    /// `=~`
    Re,
    /// `!~`: not-regex inside a filter, not-sibling between spansets.
    Nre,
    /// `>>`
    Desc,
    /// `<<`
    Ance,
    /// `~`
    Sibl,
    /// `!>`
    NotChild,
    /// `!<`
    NotParent,
    /// `!>>`
    NotDesc,
    /// `!<<`
    NotAnce,
    /// `&>`
    UnionChild,
    /// `&<`
    UnionParent,
    /// `&>>`
    UnionDesc,
    /// `&<<`
    UnionAnce,
    /// `&~`
    UnionSibl,
    /// `+ - * / % ^`: not supported, except `-` in front of a number.
    Arith(char),
    Str(String),
    Int(i64),
    Float(f64),
    /// Nanoseconds.
    Duration(u64),
    /// A bare word: a keyword (`true`, `error`, `server`...), an intrinsic
    /// (`name`, `duration`...) or an unknown word, decided by the parser.
    Ident(String),
    /// `span:name`, `trace:rootService`...
    ScopedIntrinsic {
        scope: String,
        name: String,
    },
    /// `.name`, `span.name`, `resource.name`...
    Attr {
        scope: ScopeKind,
        name: String,
    },
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

/// Lexes the whole query. The last token is always [`Tok::Eof`], positioned at
/// the end of the text.
pub(crate) fn lex(chars: &[char]) -> Vec<Token> {
    let mut lexer = Lexer { chars, pos: 0 };
    let mut tokens = Vec::new();
    loop {
        lexer.skip_whitespace();
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

/// Characters that end an unquoted attribute name (besides whitespace).
const ATTRIBUTE_TERMINATORS: &str = "{}()=~!<>&|^,";

fn is_attribute_char(c: char) -> bool {
    !c.is_whitespace() && !ATTRIBUTE_TERMINATORS.contains(c)
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

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(char::is_whitespace) {
            self.pos += 1;
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
                Some('|') => self.take(2, Tok::OrOr),
                _ => self.take(1, Tok::Pipe),
            },
            '&' => match (self.peek_at(1), self.peek_at(2)) {
                (Some('&'), _) => self.take(2, Tok::AndAnd),
                (Some('>'), Some('>')) => self.take(3, Tok::UnionDesc),
                (Some('>'), _) => self.take(2, Tok::UnionChild),
                (Some('<'), Some('<')) => self.take(3, Tok::UnionAnce),
                (Some('<'), _) => self.take(2, Tok::UnionParent),
                (Some('~'), _) => self.take(2, Tok::UnionSibl),
                _ => Err(QueryError::new(start, "unexpected character `&`")),
            },
            '=' => match self.peek_at(1) {
                Some('~') => self.take(2, Tok::Re),
                _ => self.take(1, Tok::Eq),
            },
            '!' => match (self.peek_at(1), self.peek_at(2)) {
                (Some('='), _) => self.take(2, Tok::Neq),
                (Some('~'), _) => self.take(2, Tok::Nre),
                (Some('>'), Some('>')) => self.take(3, Tok::NotDesc),
                (Some('>'), _) => self.take(2, Tok::NotChild),
                (Some('<'), Some('<')) => self.take(3, Tok::NotAnce),
                (Some('<'), _) => self.take(2, Tok::NotParent),
                _ => self.take(1, Tok::Not),
            },
            '>' => match self.peek_at(1) {
                Some('>') => self.take(2, Tok::Desc),
                Some('=') => self.take(2, Tok::Gte),
                _ => self.take(1, Tok::Gt),
            },
            '<' => match self.peek_at(1) {
                Some('<') => self.take(2, Tok::Ance),
                Some('=') => self.take(2, Tok::Lte),
                _ => self.take(1, Tok::Lt),
            },
            '~' => self.take(1, Tok::Sibl),
            '+' | '-' | '*' | '/' | '%' | '^' => self.take(1, Tok::Arith(c)),
            '"' => self.string(),
            '`' => self.raw_string(),
            '0'..='9' => self.number(),
            // `.5` is a number, as in Tempo; any other `.` starts an attribute.
            '.' if self.peek_at(1).is_some_and(|d| d.is_ascii_digit()) => self.number(),
            '.' => {
                self.pos += 1;
                let name = self.attribute_name(".")?;
                Ok(Tok::Attr {
                    scope: ScopeKind::Any,
                    name,
                })
            }
            c if c.is_alphabetic() || c == '_' => self.word(),
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

    /// A bare word, or the start of a scoped attribute (`span.x`) or a scoped
    /// intrinsic (`span:name`).
    fn word(&mut self) -> Result<Tok, QueryError> {
        let start = self.pos;
        while self.peek().is_some_and(is_word_char) {
            self.pos += 1;
        }
        let word: String = self.chars[start..self.pos].iter().collect();
        match self.peek() {
            Some('.') => {
                let scope = match word.as_str() {
                    "span" => Some(ScopeKind::Span),
                    "resource" => Some(ScopeKind::Resource),
                    "parent" => Some(ScopeKind::Parent),
                    "event" => Some(ScopeKind::Event),
                    "link" => Some(ScopeKind::Link),
                    "instrumentation" => Some(ScopeKind::Instrumentation),
                    _ => None,
                };
                if let Some(scope) = scope {
                    self.pos += 1;
                    if scope == ScopeKind::Parent {
                        // `parent.span.x` and `parent.resource.x`.
                        for inner in ["span.", "resource."] {
                            if self.starts_with(inner) {
                                self.pos += inner.chars().count();
                                break;
                            }
                        }
                    }
                    let name = self.attribute_name(&format!("{word}."))?;
                    return Ok(Tok::Attr { scope, name });
                }
            }
            Some(':')
                if matches!(
                    word.as_str(),
                    "span" | "trace" | "event" | "link" | "instrumentation"
                ) =>
            {
                self.pos += 1;
                let name_start = self.pos;
                while self.peek().is_some_and(is_word_char) {
                    self.pos += 1;
                }
                let name: String = self.chars[name_start..self.pos].iter().collect();
                return Ok(Tok::ScopedIntrinsic { scope: word, name });
            }
            _ => {}
        }
        Ok(Tok::Ident(word))
    }

    fn starts_with(&self, text: &str) -> bool {
        text.chars()
            .enumerate()
            .all(|(i, c)| self.peek_at(i) == Some(c))
    }

    /// The name after a scope prefix: unquoted runs and `"quoted"` parts.
    /// `prefix` only names the scope in the error for an empty name.
    fn attribute_name(&mut self, prefix: &str) -> Result<String, QueryError> {
        let mut name = String::new();
        loop {
            match self.peek() {
                Some('"') => name.push_str(&self.quoted_name()?),
                Some(c) if is_attribute_char(c) => {
                    name.push(c);
                    self.pos += 1;
                }
                _ => break,
            }
        }
        if name.is_empty() {
            return Err(QueryError::new(
                self.pos,
                format!("expected an attribute name after `{prefix}`"),
            ));
        }
        Ok(name)
    }

    /// A `"quoted part"` of an attribute name. Only `\"` and `\\` are escapes.
    fn quoted_name(&mut self) -> Result<String, QueryError> {
        let start = self.pos;
        self.pos += 1;
        let mut out = String::new();
        loop {
            match self.peek() {
                None => {
                    return Err(QueryError::new(start, "unterminated quoted attribute name"));
                }
                Some('"') => {
                    self.pos += 1;
                    return Ok(out);
                }
                Some('\\') => match self.peek_at(1) {
                    Some(c @ ('"' | '\\')) => {
                        out.push(c);
                        self.pos += 2;
                    }
                    Some(c) => {
                        return Err(QueryError::new(
                            self.pos,
                            format!("invalid escape sequence `\\{c}`"),
                        ));
                    }
                    None => {
                        return Err(QueryError::new(start, "unterminated quoted attribute name"));
                    }
                },
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
    }

    /// `"text"` with `\"`, `\\`, `\n`, `\t` and `\r` escapes.
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
                Some('\\') => {
                    match self.peek_at(1) {
                        Some('"') => out.push('"'),
                        Some('\\') => out.push('\\'),
                        Some('n') => out.push('\n'),
                        Some('t') => out.push('\t'),
                        Some('r') => out.push('\r'),
                        Some(c) => {
                            return Err(QueryError::new(
                                self.pos,
                                format!("invalid escape sequence `\\{c}`"),
                            ));
                        }
                        None => return Err(QueryError::new(start, "unterminated string")),
                    }
                    self.pos += 2;
                }
                Some(c) => {
                    out.push(c);
                    self.pos += 1;
                }
            }
        }
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

    /// An integer, a float or a duration. The lexer takes the whole run of
    /// digits, dots and letters (so `1h30m` and `500ms` stay in one piece),
    /// then decides what it is.
    fn number(&mut self) -> Result<Tok, QueryError> {
        let start = self.pos;
        while self
            .peek()
            .is_some_and(|c| c.is_ascii_digit() || c == '.' || c.is_alphabetic())
        {
            self.pos += 1;
        }
        let text: String = self.chars[start..self.pos].iter().collect();

        if text.chars().all(|c| c.is_ascii_digit()) {
            return match text.parse::<i64>() {
                Ok(n) => Ok(Tok::Int(n)),
                Err(_) => Err(QueryError::new(
                    start,
                    format!("integer `{text}` is out of range"),
                )),
            };
        }
        if let Some(nanos) = parse_duration(&text) {
            return Ok(Tok::Duration(nanos));
        }
        if text
            .chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E'))
            && let Ok(f) = text.parse::<f64>()
        {
            return Ok(Tok::Float(f));
        }
        let message = if text.chars().any(char::is_alphabetic) {
            format!("invalid duration `{text}` (units are ns, us, µs, ms, s, m, h)")
        } else {
            format!("invalid number `{text}`")
        };
        Err(QueryError::new(start, message))
    }
}
