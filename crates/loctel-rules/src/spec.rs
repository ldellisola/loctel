//! The rule as the user writes it: one `[[rule]]` of `rules.toml`.

use serde::{Deserialize, Serialize};

/// The query language of a rule, and so what it judges: TraceQL rules judge
/// spans (and, through them, traces), LogQL rules judge logs. In the file it
/// is written `"traceql"` or `"logql"`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleLanguage {
    TraceQl,
    LogQl,
}

/// What a TraceQL rule drops when it matches (design §7). In the file it is
/// written `"trace"` or `"span"`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DropScope {
    /// The whole trace: every span of it, stored or still to arrive, and
    /// every log that carries its trace ID. The default.
    #[default]
    Trace,
    /// Only the spans the query matched and their descendants (and the logs
    /// emitted in them).
    Span,
}

/// A discard rule, as saved. Whether the query is any good is not part of the
/// spec: a rule with a query that does not parse is kept as written and
/// reported with its error (`RuleStatus::error`), so that one typo in a
/// hand-edited file does not take the other rules with it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuleSpec {
    /// What the user calls the rule.
    pub name: String,
    /// A rule that is off is kept in the file and does nothing.
    pub enabled: bool,
    pub language: RuleLanguage,
    /// A TraceQL or LogQL query, as written in Grafana.
    pub query: String,
    /// What a TraceQL match drops. Ignored for LogQL, which only ever drops
    /// the log that matched. A rule written without it drops the whole trace.
    #[serde(default)]
    pub scope: DropScope,
}
