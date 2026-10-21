//! The in-memory store behind loctel.
//!
//! Spans arrive from several services, in batches and in any order. The
//! store assembles them into traces by trace ID, ignores exporter retries,
//! says when a trace has gone quiet (complete), and evicts the oldest trace
//! whole when it holds too many. It also works out which traces failed and
//! in which span (design 9.4, see [`classify`]), and lists and searches
//! traces as one-row [`TraceSummary`] values, a page at a time. Everything
//! goes through [`Store`]; the internals are free to change (the design
//! replaces them with a spill-to-disk segment store later) as long as that
//! API holds.
//!
//! Time comes from a [`Clock`], so tests drive completeness with a
//! [`ManualClock`] instead of sleeping.

mod clock;
mod config;
mod failure;
mod page;
mod store;
mod summary;
mod trace;

pub use clock::{Clock, ManualClock, SystemClock};
pub use config::StoreConfig;
pub use failure::{FailedIn, TraceStatus, classify, trace_exception};
pub use page::{Page, PageRequest};
pub use store::{Store, StoreStats};
pub use summary::{TraceFilter, TraceSummary};
pub use trace::TraceSnapshot;
