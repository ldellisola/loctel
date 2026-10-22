//! What the Discard rules screen shows: the cards of the list, the line with
//! the total dropped, the banners, the editor's draft with its validation and
//! the preview's sentence, and the rule "Discard lines like this…" suggests from
//! a log. Plain Rust over the rules crate's types, with no GPUI in it: every
//! exact string of the plan is pinned in `tests/model_rules.rs`.

use loctel_model::LogRecord;
use loctel_query::{logql, traceql};
use loctel_rules::{DropScope, Preview, RuleId, RuleLanguage, RuleSpec, RuleStatus};

use super::logs::escaped;
use super::trace_table::thousands;

/// The text of the card's count when the rule is switched off.
pub const OFF: &str = "Off";

/// What the total line says while nothing has been dropped.
pub const NOTHING_DROPPED: &str = "Nothing dropped yet";

/// The line under the screen's title.
pub const SUBTITLE: &str = "Anything a rule matches is dropped as it arrives and never stored. \
                            Rules are kept when loctel restarts.";

/// What the list says when there are no rules at all.
pub const NO_RULES: &str = "No discard rules. Nothing is dropped unless you add one.";

/// The title of the editor's preview box.
pub const PREVIEW_TITLE: &str = "Tried on the last 5 minutes";

/// What the preview box says while the query does not parse.
pub const PREVIEW_INVALID: &str = "Fix the query to see what it would drop.";

/// How many characters of the log's body the suggested rule's name quotes.
pub const NAME_BODY_CHARS: usize = 40;

/// How many characters of the log's body the suggested rule's filter holds at
/// the most, before it is cut back to a word boundary.
pub const FILTER_BODY_CHARS: usize = 60;

/// One card of the rules list.
#[derive(Clone, Debug, PartialEq)]
pub struct RuleRow {
    pub id: RuleId,
    /// The state of the card's switch.
    pub enabled: bool,
    pub name: String,
    /// The language chip: `TRACEQL` or `LOGQL`.
    pub chip: &'static str,
    /// The query, as written (the view shows it in mono, on one line).
    pub query: String,
    /// The text on the right of the card:
    ///
    /// - `Off` for a rule that is switched off, whatever else is true of it
    /// - `Error: <message>` for a rule that is on and whose query does not parse
    ///   (it drops nothing), in the error colour
    /// - for a trace rule `48,210 spans dropped`, and when it drops whole
    ///   traces (scope trace) and has dropped some, `48,210 spans dropped in 602
    ///   traces`
    /// - for a log rule `96,433 lines dropped`
    ///
    /// A count is singular for one (`1 span dropped in 1 trace`), plural for any
    /// other number, and has a thousands separator.
    pub counts: String,
    /// The query's error, set exactly when `counts` shows it (a rule that is
    /// on): the view paints the card's count in the error colour when it is set.
    /// A switched-off draft with a bad query reads `Off`, as in the menu's rules
    /// line, where an error only counts for a rule that is on.
    pub error: Option<String>,
}

/// The cards for `statuses`, in the same order (the file's).
pub fn rows(statuses: &[RuleStatus]) -> Vec<RuleRow> {
    statuses.iter().map(row).collect()
}

fn row(status: &RuleStatus) -> RuleRow {
    let spec = &status.spec;
    let error = status.error.clone().filter(|_| spec.enabled);
    let counts = if !spec.enabled {
        OFF.to_owned()
    } else if let Some(message) = &error {
        format!("Error: {message}")
    } else {
        match spec.language {
            RuleLanguage::TraceQl => {
                let mut text =
                    format!("{} dropped", counted(status.dropped_spans, "span", "spans"));
                if spec.scope == DropScope::Trace && status.dropped_traces > 0 {
                    text.push_str(&format!(
                        " in {}",
                        counted(status.dropped_traces, "trace", "traces")
                    ));
                }
                text
            }
            RuleLanguage::LogQl => {
                format!("{} dropped", counted(status.dropped_logs, "line", "lines"))
            }
        }
    };
    RuleRow {
        id: status.id,
        enabled: spec.enabled,
        name: spec.name.clone(),
        chip: match spec.language {
            RuleLanguage::TraceQl => "TRACEQL",
            RuleLanguage::LogQl => "LOGQL",
        },
        query: spec.query.clone(),
        counts,
        error,
    }
}

/// `1,204`: `n` with a thousands separator.
fn grouped(n: u64) -> String {
    // A count over usize::MAX cannot happen on the 64-bit targets loctel
    // builds for; saturating keeps it from wrapping anywhere else.
    thousands(usize::try_from(n).unwrap_or(usize::MAX))
}

/// `1,204 spans`, `1 span`, `0 spans`.
fn counted(n: u64, one: &str, many: &str) -> String {
    let noun = if n == 1 { one } else { many };
    format!("{} {noun}", grouped(n))
}

/// The banner of a rules file that cannot be used: `The rules file has an
/// error: <message>. loctel keeps using the rules it had.` The message is the
/// one line it is shown as: its blanks and line breaks (a TOML error is several
/// lines) become single spaces, and a full stop at its end is dropped, so the
/// sentence has one. A message with no words leaves `The rules file has an
/// error.`.
pub fn file_error_line(message: &str) -> String {
    let flat = message.split_whitespace().collect::<Vec<_>>().join(" ");
    let flat = flat.trim_end_matches('.');
    if flat.is_empty() {
        "The rules file has an error. loctel keeps using the rules it had.".to_owned()
    } else {
        format!("The rules file has an error: {flat}. loctel keeps using the rules it had.")
    }
}

/// The preview box's sentence for a query that parses, as the editor's
/// language reads it:
///
/// - a trace rule: `Would have dropped 1,204 spans in 602 traces. None of those
///   traces had errors.` (or `3 of those traces had errors.`)
/// - a log rule: `Would have dropped 96 lines. 2 were errors.`
/// - a rule that matched nothing: `Would have dropped nothing.`
///
/// Singular for one (`1 span in 1 trace. That trace had errors.`, `1 line. It was
/// an error.`), and counts have a thousands separator. The logs a trace rule
/// takes with its traces are not part of its sentence.
pub fn preview_text(language: RuleLanguage, preview: &Preview) -> String {
    match language {
        RuleLanguage::TraceQl => {
            if preview.traces == 0 {
                return NOTHING_WOULD_DROP.to_owned();
            }
            let errors = match (preview.error_traces, preview.traces) {
                (0, 1) => "That trace had no errors.".to_owned(),
                (0, _) => "None of those traces had errors.".to_owned(),
                (_, 1) => "That trace had errors.".to_owned(),
                (n, _) => format!("{} of those traces had errors.", grouped(n)),
            };
            format!(
                "Would have dropped {} in {}. {errors}",
                counted(preview.spans, "span", "spans"),
                counted(preview.traces, "trace", "traces"),
            )
        }
        RuleLanguage::LogQl => {
            if preview.logs == 0 {
                return NOTHING_WOULD_DROP.to_owned();
            }
            let errors = match (preview.error_logs, preview.logs) {
                (0, 1) => "It was not an error.".to_owned(),
                (0, _) => "None were errors.".to_owned(),
                (1, 1) => "It was an error.".to_owned(),
                (1, _) => "1 was an error.".to_owned(),
                (n, _) => format!("{} were errors.", grouped(n)),
            };
            format!(
                "Would have dropped {}. {errors}",
                counted(preview.logs, "line", "lines")
            )
        }
    }
}

/// What the preview says of a rule that would have dropped nothing.
const NOTHING_WOULD_DROP: &str = "Would have dropped nothing.";

/// The line on the right of the header: the share of everything that arrived
/// that the rules dropped, since launch.
///
/// - `31% of incoming data dropped`, as a whole percent (cut to 100 if it is
///   more)
/// - `Nothing dropped yet` for 0, and for a value that is not a number or is
///   negative
/// - `<1% of incoming data dropped` for a share that is more than nothing but
///   rounds to 0%: something was dropped, so it must not read as nothing
pub fn dropped_line(fraction: f64) -> String {
    if !fraction.is_finite() || fraction <= 0.0 {
        return NOTHING_DROPPED.to_owned();
    }
    let percent = (fraction.min(1.0) * 100.0).round() as u32;
    if percent == 0 {
        "<1% of incoming data dropped".to_owned()
    } else {
        format!("{percent}% of incoming data dropped")
    }
}

/// A rule in the editor: what the fields hold right now, saved or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Draft {
    pub name: String,
    pub language: RuleLanguage,
    pub query: String,
    /// What a TraceQL match drops. The editor shows it for traces only; it is
    /// ignored for LogQL.
    pub scope: DropScope,
    pub enabled: bool,
}

/// What the editor says about a [`Draft`] while it is edited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DraftCheck {
    /// The name has something in it.
    pub name_ok: bool,
    /// Whether the query parses with the draft's language's parser: the error is
    /// `(column, message)`, the column counted from 1 in characters of the text
    /// as typed (as in the query bars), the message without it.
    pub query: Result<(), (usize, String)>,
}

impl Draft {
    /// The draft of a saved rule.
    pub fn from_spec(spec: &RuleSpec) -> Draft {
        Draft {
            name: spec.name.clone(),
            language: spec.language,
            query: spec.query.clone(),
            scope: spec.scope,
            enabled: spec.enabled,
        }
    }

    /// The rule to save. Every field goes as typed, blanks included: the rules
    /// crate keeps a rule's identity by its name, language, query and scope, so
    /// opening a rule and saving it unchanged must change nothing.
    pub fn to_spec(&self) -> RuleSpec {
        RuleSpec {
            name: self.name.clone(),
            enabled: self.enabled,
            language: self.language,
            query: self.query.clone(),
            scope: self.scope,
        }
    }

    /// Reads the name and the query. The query goes to the parser as it is (the
    /// rules engine does the same), so an empty one is an error too.
    pub fn validate(&self) -> DraftCheck {
        let parsed = match self.language {
            RuleLanguage::TraceQl => traceql::parse(&self.query).map(|_| ()),
            RuleLanguage::LogQl => logql::parse(&self.query).map(|_| ()),
        };
        DraftCheck {
            name_ok: !self.name.trim().is_empty(),
            query: parsed.map_err(|error| (error.position + 1, error.message)),
        }
    }

    /// Whether `Save` is enabled: the name has something in it. A query that
    /// does not parse can be saved (a draft, per M2): the rule shows its error
    /// on its card and drops nothing.
    pub fn can_save(&self) -> bool {
        self.validate().name_ok
    }
}

/// The line under the editor's query input: `at column 12: <message>` (the
/// column counts from 1) while the text does not parse. `None` for a query that
/// parses and for one with nothing typed: a rule that has just been started is
/// not an error yet (the preview box and the card of a saved rule say the query
/// is not one).
pub fn draft_error_line(draft: &Draft) -> Option<String> {
    if draft.query.trim().is_empty() {
        return None;
    }
    draft
        .validate()
        .query
        .err()
        .map(|(column, message)| format!("at column {column}: {message}"))
}

/// The rule `Discard lines like this…` opens in the editor, unsaved:
///
/// - the name is `Lines like "<first 40 characters of the body>…"`, with no `…`
///   when the body fits
/// - the language is LogQL
/// - the query is `{service_name="<service>"} |= "<body text>"`: the body's
///   first 60 characters, cut back to the last word boundary when that cuts a
///   word, with `"` and `\` escaped (so it finds the line it came from)
/// - it is on, so saving it starts dropping
///
/// The body is its first line that has anything in it: a stack trace after the
/// message would make a filter no other line matches, and a break in a name or
/// a filter would not be one line. A body with no text at all gives the
/// selector alone. Lengths count characters, never bytes.
pub fn suggest_from_log(record: &LogRecord) -> Draft {
    let line = record
        .body
        .split(['\n', '\r'])
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let quoted = if line.chars().nth(NAME_BODY_CHARS).is_some() {
        // Longer than 40: the first 40 and then an ellipsis.
        let mut cut: String = line.chars().take(NAME_BODY_CHARS).collect();
        cut.push('\u{2026}');
        cut
    } else {
        line.to_owned()
    };
    let text = filter_text(line);
    let selector = format!("{{service_name=\"{}\"}}", escaped(record.service()));
    let query = if text.is_empty() {
        selector
    } else {
        format!("{selector} |= \"{}\"", escaped(&text))
    };
    Draft {
        name: format!("Lines like \"{quoted}\""),
        language: RuleLanguage::LogQl,
        query,
        scope: DropScope::default(),
        enabled: true,
    }
}

/// The body text of the suggested filter: `line` up to [`FILTER_BODY_CHARS`]
/// characters, cut back to the last whitespace when the cut falls inside a word,
/// without trailing blanks. A single word longer than the limit is cut at the
/// limit.
fn filter_text(line: &str) -> String {
    let window: Vec<char> = line.chars().take(FILTER_BODY_CHARS).collect();
    let next = line.chars().nth(FILTER_BODY_CHARS);
    let cut_inside_a_word = match (window.last(), next) {
        (Some(last), Some(next)) => !last.is_whitespace() && !next.is_whitespace(),
        // Nothing follows the window (the line fits), or there is no window.
        _ => false,
    };
    let kept = if cut_inside_a_word {
        match window.iter().rposition(|c| c.is_whitespace()) {
            Some(boundary) => &window[..boundary],
            // One long word: there is no boundary to go back to.
            None => &window[..],
        }
    } else {
        &window[..]
    };
    kept.iter().collect::<String>().trim_end().to_owned()
}
