//! The LogQL evaluator: does a log record match a query.
//!
//! The selector reads the record's labels directly, without building the whole
//! label set. The set (a copy, with what the parsers extract added to it) is
//! built only when the first `| json` or `| logfmt` runs, once per `matches`
//! call, and never shared between calls. Label filters before the first parser
//! read the record directly too. So a rule such as
//! `{service_name="web"} |= "GET /health"` allocates nothing.
//!
//! `matches` cannot fail: once a query has parsed, a comparison that makes no
//! sense (a number filter on a label that is not a number) is simply false.

use std::borrow::Cow;

use loctel_model::LogRecord;

use super::ast::{LabelFilter, LogQuery, Stage};
use super::labels::{self, LabelSet};
use crate::parse_duration;

impl LogQuery {
    /// Whether `log` matches the query: every selector matcher holds, and the
    /// log gets through every stage of the pipeline, in order.
    ///
    /// `| json` and `| logfmt` never drop a log: a body that is not JSON just
    /// gets the `__error__` label (so `| json | __error__ = ""` drops it).
    pub fn matches(&self, log: &LogRecord) -> bool {
        for matcher in &self.matchers {
            let value = labels::lookup(log, &matcher.label);
            if !matcher.matcher.matches(value.as_deref().unwrap_or("")) {
                return false;
            }
        }
        // `None` until a parser runs: until then, labels are read off the record.
        let mut parsed: Option<LabelSet> = None;
        for stage in &self.stages {
            match stage {
                Stage::LineFilter(filter) => {
                    if !filter.matches(&log.body) {
                        return false;
                    }
                }
                Stage::Json => {
                    let set = parsed.get_or_insert_with(|| LabelSet::new(log));
                    labels::extract_json(&log.body, set);
                }
                Stage::Logfmt => {
                    let set = parsed.get_or_insert_with(|| LabelSet::new(log));
                    labels::extract_logfmt(&log.body, set);
                }
                Stage::LabelFilter(filter) => {
                    let holds = match &parsed {
                        Some(set) => filter.holds(&|name| set.get(name).map(Cow::Borrowed)),
                        None => filter.holds(&|name| labels::lookup(log, name)),
                    };
                    if !holds {
                        return false;
                    }
                }
            }
        }
        true
    }
}

impl LabelFilter {
    /// Evaluates the filter, reading labels through `get` (`None` for a label
    /// that does not exist).
    fn holds<'a>(&self, get: &dyn Fn(&str) -> Option<Cow<'a, str>>) -> bool {
        match self {
            LabelFilter::String(matcher) => matcher
                .matcher
                .matches(get(&matcher.label).as_deref().unwrap_or("")),
            LabelFilter::Number { label, op, value } => get(label)
                .and_then(|text| label_number(&text))
                .is_some_and(|label_value| op.holds(label_value, *value)),
            LabelFilter::Duration { label, op, nanos } => get(label)
                .and_then(|text| label_duration(&text))
                .is_some_and(|label_value| op.holds(label_value, *nanos)),
            LabelFilter::And(left, right) => left.holds(get) && right.holds(get),
            LabelFilter::Or(left, right) => left.holds(get) || right.holds(get),
        }
    }
}

/// A label's text as a number, the way Go's `strconv.ParseFloat` reads it
/// (which is what Loki uses): a decimal, with an optional sign and exponent,
/// `inf`, `infinity` and `nan` in any case, and `_` between digits (`1_000`).
fn label_number(text: &str) -> Option<f64> {
    if !text.contains('_') {
        return text.parse().ok();
    }
    // Underscores are only allowed between two digits.
    let bytes = text.as_bytes();
    let between_digits = bytes.iter().enumerate().all(|(i, &b)| {
        b != b'_'
            || (i > 0
                && bytes[i - 1].is_ascii_digit()
                && bytes.get(i + 1).is_some_and(u8::is_ascii_digit))
    });
    if !between_digits {
        return None;
    }
    text.replace('_', "").parse().ok()
}

/// A label's text as a duration, in nanoseconds: `12ms`, `1.5s`, `1h30m`, with
/// an optional sign, and a bare `0` (the one number Go's `time.ParseDuration`
/// takes without a unit).
fn label_duration(text: &str) -> Option<i128> {
    let (negative, digits) = match text.as_bytes().first() {
        Some(b'-') => (true, &text[1..]),
        Some(b'+') => (false, &text[1..]),
        _ => (false, text),
    };
    if digits == "0" {
        return Some(0);
    }
    let nanos = i128::from(parse_duration(digits)?);
    Some(if negative { -nanos } else { nanos })
}
