//! Discard rules for loctel (design §7): data that matches a rule is dropped
//! when it arrives, for good.
//!
//! - [`RuleSpec`] is a rule as the user writes it, and [`RulesFile`] reads and
//!   writes `rules.toml`, the human-readable file the rules live in (a new
//!   file holds no rules: loctel never decides what to discard).
//! - [`RuleSet`] is the rules at work. Every batch of spans and logs goes
//!   through it on its way to the store: [`RuleSet::filter_spans`] and
//!   [`RuleSet::filter_logs`]. It remembers for a while what it dropped, so
//!   that the rest of a dropped trace is dropped as it arrives, and it counts
//!   what each rule dropped ([`RuleStatus`]).
//! - [`preview`] says what a rule would have dropped of the data the store
//!   holds, before the rule is saved.
//!
//! The queries are TraceQL and LogQL (`loctel-query`). See
//! `docs/areas/rules.md` for the semantics and the reasons.

mod file;
mod memory;
mod preview;
mod set;
mod spec;

pub use file::RulesFile;
pub use preview::{PREVIEW_WINDOW, Preview, preview};
pub use set::{DROPPED_MAX, DROPPED_TTL, DroppedTotals, RuleId, RuleSet, RuleStatus, SpanVerdict};
pub use spec::{DropScope, RuleLanguage, RuleSpec};
