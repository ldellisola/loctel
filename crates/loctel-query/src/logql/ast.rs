//! The LogQL syntax tree.
//!
//! Public so later tooling (the rules screen, query suggestions) can walk it,
//! but it promises nothing beyond what the plan names (`LogQuery`, `matches`,
//! `service_hint`). It is a tree of what was written, minus the punctuation:
//! `,`, a space and `and` between label filters are all [`LabelFilter::And`],
//! and parentheses leave no node.

use crate::Pattern;

/// A parsed log query: `{selector} stage stage ...`.
#[derive(Clone, Debug, PartialEq)]
pub struct LogQuery {
    /// The stream selector's matchers. All of them must hold; there is at
    /// least one.
    pub matchers: Vec<LabelMatcher>,
    /// What follows the selector, in order.
    pub stages: Vec<Stage>,
}

/// `label op "value"`, in a selector or in a label filter.
#[derive(Clone, Debug, PartialEq)]
pub struct LabelMatcher {
    pub label: String,
    pub matcher: StringMatcher,
}

/// The four string comparisons. A label that is missing compares as the empty
/// string, as in Prometheus and Loki.
#[derive(Clone, Debug, PartialEq)]
pub enum StringMatcher {
    /// `=`
    Equal(String),
    /// `!=`
    NotEqual(String),
    /// `=~`: fully anchored.
    Regex(Pattern),
    /// `!~`: fully anchored.
    NotRegex(Pattern),
}

impl StringMatcher {
    /// Whether `value` satisfies the matcher.
    pub fn matches(&self, value: &str) -> bool {
        match self {
            StringMatcher::Equal(text) => value == text,
            StringMatcher::NotEqual(text) => value != text,
            StringMatcher::Regex(pattern) => pattern.is_match(value),
            StringMatcher::NotRegex(pattern) => !pattern.is_match(value),
        }
    }
}

/// One step of the pipeline after the selector.
#[derive(Clone, Debug, PartialEq)]
pub enum Stage {
    /// `|=`, `!=`, `|~`, `!~` against the log line (the body).
    LineFilter(LineFilter),
    /// `| json`: adds the body's JSON fields as labels.
    Json,
    /// `| logfmt`: adds the body's `key=value` pairs as labels.
    Logfmt,
    /// `| label_expr`: keeps the log when the expression holds.
    LabelFilter(LabelFilter),
}

/// A filter on the log line.
#[derive(Clone, Debug, PartialEq)]
pub enum LineFilter {
    /// `|= "text"`: the line contains it.
    Contains(String),
    /// `!= "text"`: the line does not contain it.
    NotContains(String),
    /// `|~ "re"`: unanchored, matches anywhere in the line.
    Regex(Pattern),
    /// `!~ "re"`: unanchored; the line has no match.
    NotRegex(Pattern),
}

impl LineFilter {
    /// Whether the line passes the filter.
    pub fn matches(&self, line: &str) -> bool {
        match self {
            LineFilter::Contains(text) => line.contains(text.as_str()),
            LineFilter::NotContains(text) => !line.contains(text.as_str()),
            LineFilter::Regex(pattern) => pattern.is_match(line),
            LineFilter::NotRegex(pattern) => !pattern.is_match(line),
        }
    }
}

/// A numeric comparison operator: `=` and `==` (the same), `!=`, `>`, `>=`,
/// `<`, `<=`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NumberOp {
    Equal,
    NotEqual,
    Greater,
    GreaterOrEqual,
    Less,
    LessOrEqual,
}

impl NumberOp {
    /// Applies the operator to two values that compare (neither is NaN).
    pub(crate) fn holds<T: PartialOrd>(self, left: T, right: T) -> bool {
        match self {
            NumberOp::Equal => left == right,
            NumberOp::NotEqual => left != right,
            NumberOp::Greater => left > right,
            NumberOp::GreaterOrEqual => left >= right,
            NumberOp::Less => left < right,
            NumberOp::LessOrEqual => left <= right,
        }
    }
}

/// A label filter expression (the part after `| ` that is not a parser).
#[derive(Clone, Debug, PartialEq)]
pub enum LabelFilter {
    /// `label =`, `!=`, `=~` or `!~` a string.
    String(LabelMatcher),
    /// `label` compared with a number. The label's text is read as a number;
    /// if it is missing or is not one, the filter is false.
    Number {
        label: String,
        op: NumberOp,
        value: f64,
    },
    /// `label` compared with a duration, in nanoseconds (negative allowed).
    /// The label's text is read as a duration (`12ms`, `1h30m`); if it is
    /// missing or is not one, the filter is false.
    Duration {
        label: String,
        op: NumberOp,
        nanos: i128,
    },
    /// `a and b`, `a, b`, `a b`.
    And(Box<LabelFilter>, Box<LabelFilter>),
    /// `a or b`.
    Or(Box<LabelFilter>, Box<LabelFilter>),
}

impl LogQuery {
    /// The value of an exact `service_name="..."` selector matcher, if there is
    /// one. Every log the query matches comes from that service, so the store
    /// can use it to skip every other service without evaluating anything. A
    /// regex, a negation, or any other label gives `None`.
    pub fn service_hint(&self) -> Option<&str> {
        self.matchers.iter().find_map(|m| match &m.matcher {
            StringMatcher::Equal(value) if m.label == "service_name" => Some(value.as_str()),
            _ => None,
        })
    }
}
