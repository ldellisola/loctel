//! The label set of a log record, and the labels the parsers add.
//!
//! Loki's labels are what its queries read; loctel gets them from OTLP the way
//! Loki's OTLP ingest does (design §6.2):
//!
//! - every resource attribute and every log attribute, under its key with each
//!   character outside `[A-Za-z0-9_]` turned into `_` ([`loki_label_name`]);
//! - `service_name`, the service (`unknown_service` when it has no name);
//! - `level`, the severity (`trace`, `debug`, `info`, `warn`, `error`, `fatal`,
//!   or `unknown`);
//! - `scope_name`, the instrumentation scope's name, when it has one;
//! - `trace_id` and `span_id`, as lowercase hex, when the log has them.
//!
//! Those five computed labels win over an attribute of the same mapped name; a
//! resource attribute wins over a log attribute; of two attributes with one
//! mapped name the first wins. **A label with an empty value does not exist**,
//! as in Prometheus and Loki, which is also why a missing label compares as the
//! empty string.
//!
//! `| json` and `| logfmt` add labels on top (see [`LabelSet`]).

use std::borrow::Cow;
use std::collections::BTreeMap;

use loctel_model::{AttrValue, LogRecord, loki_label_name};

/// What an extracted label whose name is already a label of the log gets added
/// to its name (Loki's `_extracted`).
const EXTRACTED_SUFFIX: &str = "_extracted";

/// The label that says a parser could not read the line. Loki's name; a query
/// drops the lines that failed with `| __error__ = ""`.
const ERROR_LABEL: &str = "__error__";

/// How deep `| json` follows nested objects (and skips nested arrays) before it
/// gives up on the line, so a hostile body cannot overflow the stack.
const MAX_JSON_DEPTH: usize = 128;

/// The log's label set, sorted by name, one entry per name.
///
/// The set is what the label filters and the selector see; see the module
/// docs for what is in it and which label wins a clash.
pub fn labels(log: &LogRecord) -> Vec<(String, String)> {
    let mut set: BTreeMap<String, String> = BTreeMap::new();
    // Resource first, then the log's own attributes: the first non-empty
    // value of a mapped name stays.
    for (key, value) in log.resource.attributes.iter().chain(&log.attributes) {
        let text = value_text(value);
        if !text.is_empty() {
            set.entry(loki_label_name(key))
                .or_insert_with(|| text.into_owned());
        }
    }
    // The computed labels override.
    set.insert("service_name".to_owned(), log.service().to_owned());
    set.insert("level".to_owned(), log.severity.level_label().to_owned());
    if !log.scope_name.is_empty() {
        set.insert("scope_name".to_owned(), log.scope_name.clone());
    }
    if let Some(id) = &log.trace_id {
        set.insert("trace_id".to_owned(), id.to_string());
    }
    if let Some(id) = &log.span_id {
        set.insert("span_id".to_owned(), id.to_string());
    }
    set.into_iter().collect()
}

/// The value of one label of `log`, read straight from the record without
/// building the whole set. It gives the same answer as looking the name up in
/// [`labels`] (`None` for a missing label and for an empty one), and it is what
/// the selector uses: a rule that only reads `service_name` and the line should
/// not cost an allocation per attribute on every log.
pub(crate) fn lookup<'a>(log: &'a LogRecord, name: &str) -> Option<Cow<'a, str>> {
    let computed: Option<Cow<'a, str>> = match name {
        "service_name" => Some(Cow::Borrowed(log.service())),
        "level" => Some(Cow::Borrowed(log.severity.level_label())),
        "scope_name" if !log.scope_name.is_empty() => Some(Cow::Borrowed(log.scope_name.as_str())),
        "trace_id" => log.trace_id.as_ref().map(|id| Cow::Owned(id.to_string())),
        "span_id" => log.span_id.as_ref().map(|id| Cow::Owned(id.to_string())),
        _ => None,
    };
    if computed.is_some() {
        return computed;
    }
    log.resource
        .attributes
        .iter()
        .chain(&log.attributes)
        .filter(|(key, _)| mapped_name_is(key, name))
        .map(|(_, value)| value_text(value))
        .find(|text| !text.is_empty())
}

/// Whether `loki_label_name(key) == name`, without building the string.
fn mapped_name_is(key: &str, name: &str) -> bool {
    let mut wanted = name.chars();
    for c in key.chars() {
        let mapped = if c.is_ascii_alphanumeric() || c == '_' {
            c
        } else {
            '_'
        };
        if wanted.next() != Some(mapped) {
            return false;
        }
    }
    wanted.next().is_none()
}

/// An attribute value as label text: strings as they are, everything else in
/// its display form (numbers and booleans as text, bytes as hex, arrays and
/// maps as compact JSON).
fn value_text(value: &AttrValue) -> Cow<'_, str> {
    match value {
        AttrValue::Str(text) => Cow::Borrowed(text.as_str()),
        other => Cow::Owned(other.to_display_string()),
    }
}

/// A log's labels while a pipeline with parsers runs: the log's own set, plus
/// what `| json` and `| logfmt` extracted from the line. It is a copy, made
/// when the first parser runs, so one log's extraction never reaches the next.
pub(crate) struct LabelSet {
    /// [`labels`], sorted by name.
    base: Vec<(String, String)>,
    extracted: BTreeMap<String, String>,
}

impl LabelSet {
    pub(crate) fn new(log: &LogRecord) -> LabelSet {
        LabelSet {
            base: labels(log),
            extracted: BTreeMap::new(),
        }
    }

    /// The value of `name`, if the label exists.
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        if let Some(value) = self.extracted.get(name) {
            return Some(value.as_str());
        }
        self.base
            .binary_search_by(|(key, _)| key.as_str().cmp(name))
            .ok()
            .map(|i| self.base[i].1.as_str())
    }

    /// Adds an extracted label. A name that is already a label of the log gets
    /// `_extracted` after it, as in Loki, so a field called `level` in the body
    /// does not shadow the log's level. The first value extracted for a name
    /// stays, whether a later one comes from the same line or from a later
    /// parser (Loki records what it extracted and skips it afterwards). An
    /// empty value is no label.
    fn extract(&mut self, key: String, value: String) {
        if value.is_empty() {
            return;
        }
        let clashes = self
            .base
            .binary_search_by(|(name, _)| name.as_str().cmp(&key))
            .is_ok();
        let key = if clashes {
            format!("{key}{EXTRACTED_SUFFIX}")
        } else {
            key
        };
        self.extracted.entry(key).or_insert(value);
    }

    fn set_error(&mut self, kind: &str) {
        self.extracted
            .insert(ERROR_LABEL.to_owned(), kind.to_owned());
    }
}

/// A label name for a parsed key: [`loki_label_name`], with `_` in front of a
/// leading digit, and none for an empty key (Loki's `sanitizeLabelKey`).
fn sanitize_key(raw: &str) -> Option<String> {
    let name = loki_label_name(raw.trim());
    match name.chars().next() {
        None => None,
        Some(first) if first.is_ascii_digit() => Some(format!("_{name}")),
        Some(_) => Some(name),
    }
}

// ---- | json --------------------------------------------------------------------

/// `| json`: adds the body's fields to `set`.
///
/// The body must be one JSON object. String, number and boolean fields become
/// labels, named by their key, with nested objects flattened by joining the
/// keys with `_` (`{"user":{"id":42}}` gives `user_id="42"`). Arrays and
/// nulls are skipped, and numbers keep the text they have in the body
/// (`129.00` stays `129.00`). A body that is not a JSON object, or breaks off
/// or goes wrong half way, adds `__error__="JSONParserErr"` (the labels read
/// before the problem stay) and the log is kept.
///
/// This follows Loki's own scanner (`jsonparser`) where it is lenient, because
/// a body that is half JSON should give the labels Loki gives it: anything
/// after the closing `}` is ignored, a trailing comma is fine, arrays are
/// skipped by matching their brackets without looking inside, a number is
/// whatever runs up to the next delimiter, and a string with a bad escape is a
/// field with an empty value (so no label).
pub(crate) fn extract_json(body: &str, set: &mut LabelSet) {
    let mut scanner = Json {
        bytes: body.as_bytes(),
        text: body,
        pos: 0,
        path: Vec::new(),
        set,
    };
    if scanner.document().is_err() {
        scanner.set.set_error("JSONParserErr");
    }
}

/// A JSON syntax error. It carries nothing: the label says all there is to say.
struct JsonError;

struct Json<'a, 'b> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    /// The raw keys of the objects around the cursor.
    path: Vec<String>,
    set: &'b mut LabelSet,
}

impl Json<'_, '_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn skip_whitespace(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n' | b'\r')) {
            self.pos += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Result<(), JsonError> {
        if self.peek() == Some(byte) {
            self.pos += 1;
            Ok(())
        } else {
            Err(JsonError)
        }
    }

    /// The body: one object. What comes after it is not looked at.
    fn document(&mut self) -> Result<(), JsonError> {
        self.skip_whitespace();
        if self.peek() != Some(b'{') {
            return Err(JsonError);
        }
        self.object(1)
    }

    /// An object, from its `{`. `depth` counts it.
    fn object(&mut self, depth: usize) -> Result<(), JsonError> {
        self.pos += 1;
        self.skip_whitespace();
        if self.peek() == Some(b'}') {
            self.pos += 1;
            return Ok(());
        }
        loop {
            self.skip_whitespace();
            let raw = self.string_raw()?;
            let key = unescape(raw).ok_or(JsonError)?;
            self.skip_whitespace();
            self.expect(b':')?;
            self.skip_whitespace();
            self.field(key, depth)?;
            self.skip_whitespace();
            match self.peek() {
                Some(b',') => {
                    self.pos += 1;
                    self.skip_whitespace();
                    // A trailing comma.
                    if self.peek() == Some(b'}') {
                        self.pos += 1;
                        return Ok(());
                    }
                }
                Some(b'}') => {
                    self.pos += 1;
                    return Ok(());
                }
                _ => return Err(JsonError),
            }
        }
    }

    /// The value of the field `key`.
    fn field(&mut self, key: String, depth: usize) -> Result<(), JsonError> {
        match self.peek() {
            Some(b'"') => {
                // A bad escape makes the value empty, which is no label.
                let value = unescape(self.string_raw()?).unwrap_or_default();
                self.leaf(&key, value);
            }
            Some(b'{') => {
                if depth >= MAX_JSON_DEPTH {
                    return Err(JsonError);
                }
                self.path.push(key);
                let result = self.object(depth + 1);
                self.path.pop();
                result?;
            }
            Some(b'[') => self.skip_array()?,
            Some(b'-' | b'0'..=b'9') => {
                let start = self.pos;
                while !matches!(
                    self.peek(),
                    None | Some(b',' | b'}' | b']' | b' ' | b'\t' | b'\n' | b'\r')
                ) {
                    self.pos += 1;
                }
                let value = self.text[start..self.pos].to_owned();
                self.leaf(&key, value);
            }
            Some(b't') => {
                self.literal("true")?;
                self.leaf(&key, "true".to_owned());
            }
            Some(b'f') => {
                self.literal("false")?;
                self.leaf(&key, "false".to_owned());
            }
            Some(b'n') => self.literal("null")?,
            _ => return Err(JsonError),
        }
        Ok(())
    }

    /// Adds the label for a scalar field: the keys around it, joined by `_`
    /// (a blank key is left out).
    fn leaf(&mut self, key: &str, value: String) {
        let mut joined = String::new();
        for part in self.path.iter().map(String::as_str).chain([key]) {
            if part.trim().is_empty() {
                continue;
            }
            if !joined.is_empty() {
                joined.push('_');
            }
            joined.push_str(part);
        }
        if let Some(name) = sanitize_key(&joined) {
            self.set.extract(name, value);
        }
    }

    fn literal(&mut self, word: &str) -> Result<(), JsonError> {
        if self.bytes[self.pos..].starts_with(word.as_bytes()) {
            self.pos += word.len();
            Ok(())
        } else {
            Err(JsonError)
        }
    }

    /// A string, from its opening quote: the text between the quotes as it is
    /// written (escapes not yet resolved). The closing quote is the first one
    /// that no backslash escapes.
    fn string_raw(&mut self) -> Result<&str, JsonError> {
        self.expect(b'"')?;
        let start = self.pos;
        // Only `"` and `\` matter, and both are ASCII, so the slice starts and
        // ends on a character boundary.
        loop {
            match self.peek() {
                None => return Err(JsonError),
                Some(b'"') => {
                    let raw = &self.text[start..self.pos];
                    self.pos += 1;
                    return Ok(raw);
                }
                Some(b'\\') => self.pos += 2,
                Some(_) => self.pos += 1,
            }
        }
    }

    /// Reads past an array, from its `[`, by matching brackets and stepping
    /// over strings, without looking at what is inside.
    fn skip_array(&mut self) -> Result<(), JsonError> {
        let mut level = 0usize;
        loop {
            match self.peek() {
                None => return Err(JsonError),
                Some(b'"') => {
                    self.string_raw()?;
                    continue;
                }
                Some(b'[') => level += 1,
                Some(b']') => {
                    level -= 1;
                    if level == 0 {
                        self.pos += 1;
                        return Ok(());
                    }
                }
                Some(_) => {}
            }
            self.pos += 1;
        }
    }
}

/// A JSON string's text with its escapes resolved (`\" \\ \/ \b \f \n \r \t`
/// and `\uXXXX`, with surrogate pairs), or `None` for an escape that is not
/// valid, including a surrogate with no partner.
fn unescape(raw: &str) -> Option<String> {
    if !raw.contains('\\') {
        return Some(raw.to_owned());
    }
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '"' => out.push('"'),
            '\\' => out.push('\\'),
            '/' => out.push('/'),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'u' => {
                let first = hex4(&mut chars)?;
                let code = match first {
                    0xd800..=0xdbff => {
                        // A high surrogate needs `\uXXXX` with a low one.
                        if chars.next()? != '\\' || chars.next()? != 'u' {
                            return None;
                        }
                        let second = hex4(&mut chars)?;
                        if !(0xdc00..=0xdfff).contains(&second) {
                            return None;
                        }
                        0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00)
                    }
                    0xdc00..=0xdfff => return None,
                    other => other,
                };
                out.push(char::from_u32(code)?);
            }
            _ => return None,
        }
    }
    Some(out)
}

/// Four hex digits.
fn hex4(chars: &mut std::str::Chars<'_>) -> Option<u32> {
    let mut value = 0;
    for _ in 0..4 {
        value = value * 16 + chars.next()?.to_digit(16)?;
    }
    Some(value)
}

// ---- | logfmt -------------------------------------------------------------------

/// `| logfmt`: adds the body's `key=value` pairs to `set`.
///
/// This is Loki's decoder (`pkg/logql/log/logfmt/decode.go`) in its default,
/// lenient mode, so a line gives the labels it gives there:
///
/// - pairs are separated by whitespace (any byte up to a space);
/// - a value is either `"quoted"` (with the escapes `\" \\ \/ \' \b \f \n \r
///   \t \uXXXX`) or runs to the next whitespace;
/// - a key with no value, or an empty one, is skipped;
/// - a pair that breaks the format is dropped with the rest of its word: an
///   empty key (`=x`), a quote in a key or in an unquoted value, an `=` in an
///   unquoted value (`path=/a?x=1`), an escape that does not exist inside quotes.
///   A quote that never closes drops the rest of the line;
/// - keys are made into label names the way `| json` does it.
///
/// A line that is not logfmt adds nothing and is no error.
pub(crate) fn extract_logfmt(body: &str, set: &mut LabelSet) {
    let mut pos = 0;
    loop {
        match scan_keyval(body, &mut pos) {
            Scanned::End => return,
            Scanned::Dropped => {}
            Scanned::Pair { key, value } => {
                if !value.is_empty()
                    && let Some(name) = sanitize_key(key)
                {
                    set.extract(name, value.into_owned());
                }
            }
        }
    }
}

enum Scanned<'a> {
    /// No more pairs.
    End,
    /// A pair that breaks the format; the cursor is past it.
    Dropped,
    /// A key and its value (empty when it has none).
    Pair { key: &'a str, value: Cow<'a, str> },
}

/// The next pair from `pos`, a port of the Go decoder's `ScanKeyval`. All the
/// bytes it tests for are ASCII, so every slice is on a character boundary.
fn scan_keyval<'a>(body: &'a str, pos: &mut usize) -> Scanned<'a> {
    let bytes = body.as_bytes();
    let n = bytes.len();
    while *pos < n && bytes[*pos] <= b' ' {
        *pos += 1;
    }
    if *pos >= n {
        return Scanned::End;
    }
    // The key runs to `=`, a quote, whitespace or the end of the line.
    let start = *pos;
    let mut i = start;
    while i < n && bytes[i] != b'=' && bytes[i] != b'"' && bytes[i] > b' ' {
        i += 1;
    }
    *pos = i;
    if i >= n || bytes[i] <= b' ' {
        return Scanned::Pair {
            key: &body[start..i],
            value: Cow::Borrowed(""),
        };
    }
    if bytes[i] == b'"' || i == start {
        // A quote in the key, or `=` with no key.
        return skip_word(bytes, pos);
    }
    let key = &body[start..i];
    // After the `=`.
    *pos += 1;
    if *pos >= n || bytes[*pos] <= b' ' {
        return Scanned::Pair {
            key,
            value: Cow::Borrowed(""),
        };
    }
    if bytes[*pos] == b'"' {
        return scan_quoted(body, pos, key);
    }
    let value_start = *pos;
    let mut i = value_start;
    while i < n && bytes[i] > b' ' {
        if bytes[i] == b'=' || bytes[i] == b'"' {
            *pos = i;
            return skip_word(bytes, pos);
        }
        i += 1;
    }
    *pos = i;
    Scanned::Pair {
        key,
        value: Cow::Borrowed(&body[value_start..i]),
    }
}

/// Drops the rest of the word the cursor is in.
fn skip_word<'a>(bytes: &[u8], pos: &mut usize) -> Scanned<'a> {
    while *pos < bytes.len() && bytes[*pos] > b' ' {
        *pos += 1;
    }
    Scanned::Dropped
}

/// A quoted value, from its opening quote.
fn scan_quoted<'a>(body: &'a str, pos: &mut usize, key: &'a str) -> Scanned<'a> {
    let bytes = body.as_bytes();
    let open = *pos;
    let mut escaped = false;
    let mut has_escape = false;
    for (offset, &byte) in bytes[open + 1..].iter().enumerate() {
        if escaped {
            escaped = false;
        } else if byte == b'\\' {
            has_escape = true;
            escaped = true;
        } else if byte == b'"' {
            let close = open + 1 + offset;
            *pos = close + 1;
            let inner = &body[open + 1..close];
            return if has_escape {
                match unquote(inner) {
                    Some(value) => Scanned::Pair {
                        key,
                        value: Cow::Owned(value),
                    },
                    None => Scanned::Dropped,
                }
            } else {
                Scanned::Pair {
                    key,
                    value: Cow::Borrowed(inner),
                }
            };
        }
    }
    // Never closed: the rest of the line goes with it.
    *pos = bytes.len();
    Scanned::Dropped
}

/// The text of a quoted logfmt value with its escapes resolved (`\" \\ \/ \'
/// \b \f \n \r \t \uXXXX`), or `None` for an escape that does not exist.
/// A surrogate with no partner is the replacement character here, as in Go's
/// decoder (JSON strings are stricter, see [`unescape`]).
fn unquote(inner: &str) -> Option<String> {
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            c @ ('"' | '\\' | '/' | '\'') => out.push(c),
            'b' => out.push('\u{8}'),
            'f' => out.push('\u{c}'),
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'u' => {
                let first = hex4(&mut chars)?;
                if matches!(first, 0xd800..=0xdfff) {
                    // Look ahead for the other half without consuming it
                    // unless it pairs.
                    let mut ahead = chars.clone();
                    let pair = (ahead.next() == Some('\\') && ahead.next() == Some('u'))
                        .then(|| hex4(&mut ahead))
                        .flatten()
                        .filter(|second| {
                            (0xd800..=0xdbff).contains(&first) && (0xdc00..=0xdfff).contains(second)
                        });
                    match pair {
                        Some(second) => {
                            chars = ahead;
                            let code = 0x10000 + ((first - 0xd800) << 10) + (second - 0xdc00);
                            out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        }
                        None => out.push('\u{fffd}'),
                    }
                } else {
                    out.push(char::from_u32(first)?);
                }
            }
            _ => return None,
        }
    }
    Some(out)
}
