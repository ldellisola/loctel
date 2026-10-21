//! Errors from the lexers and parsers.

use std::fmt;

/// A query that could not be parsed, and where.
///
/// `position` is a 0-based **character** index into the query text (not a
/// byte offset), so it stays right for non-ASCII queries. It may equal the
/// number of characters: that means "at the end of the query". `Display`
/// counts from 1, the way an editor shows columns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryError {
    pub message: String,
    pub position: usize,
}

impl QueryError {
    pub(crate) fn new(position: usize, message: impl Into<String>) -> Self {
        QueryError {
            message: message.into(),
            position,
        }
    }

    /// `"<feature> is not supported yet"`: a recognised feature that loctel
    /// does not implement, which is an error and never a silent mismatch.
    pub(crate) fn unsupported(position: usize, feature: &str) -> Self {
        QueryError::new(position, format!("{feature} is not supported yet"))
    }
}

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "at column {}: {}", self.position + 1, self.message)
    }
}

impl std::error::Error for QueryError {}

/// The one-line reason a regex failed to compile. The `regex` crate's own
/// `Display` is several lines with a caret drawn under the pattern; the last
/// line (`error: unclosed group`) is the part worth showing next to a column.
pub(crate) fn regex_error_message(err: &regex::Error) -> String {
    let full = err.to_string();
    let reason = full
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("error: "))
        .unwrap_or(full.trim());
    format!("invalid regular expression: {reason}")
}
