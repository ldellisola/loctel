//! Query languages for loctel: TraceQL (traces) and LogQL (logs).
//!
//! Pure Rust, depending only on `loctel-model`. Syntax and semantics follow
//! Grafana's Tempo and Loki, so queries pasted from Grafana work unchanged
//! (design §6); where loctel does not support a feature the parser says so
//! with the position of the feature, and never matches wrongly in silence.
//! `docs/areas/query.md` has the supported subset, the precedence rules and
//! the reasons.
//!
//! This crate holds what both languages share:
//!
//! - [`QueryError`]: a message and the character position it points at.
//! - [`Pattern`]: a compiled regex that is either anchored (TraceQL, LogQL
//!   label matchers) or not (LogQL line filters).
//! - [`parse_duration`]: duration literals such as `500ms` and `1h30m`.
//!
//! and the language modules, starting with [`traceql`].

mod duration;
mod error;
mod pattern;
pub mod traceql;

pub use duration::parse_duration;
pub use error::QueryError;
pub use pattern::Pattern;
