//! The in-memory store behind loctel.
//!
//! Spans arrive from several services, in batches and in any order. The
//! store assembles them into traces by trace ID, ignores exporter retries,
//! says when a trace has gone quiet (complete), and evicts the oldest trace
//! whole when it holds too many. It also works out which traces failed and
//! in which span (design 9.4, see [`classify`]), and lists and searches
//! traces as one-row [`TraceSummary`] values, a page at a time.
//!
//! Log records are stored in time order, searched newest first and linked to
//! their spans. The service map and the services list are computed from the
//! traces held.
//!
//! Searches can carry a parsed TraceQL or LogQL query
//! ([`TraceFilter::query`], [`LogFilter::query`]); a matching trace lists the
//! spans the query matched. Stored traces, spans and logs can be removed again
//! ([`Store::remove_trace`], [`Store::remove_spans`]) and read in place
//! ([`Store::with_trace_spans`], [`Store::scan_traces`], [`Store::scan_logs`]),
//! which is what the discard rules are built on.
//!
//! Everything goes through [`Store`]; the internals are free to change (the
//! design replaces them with a spill-to-disk segment store later) as long as
//! that API holds.
//!
//! Time comes from a [`Clock`], so tests drive completeness with a
//! [`ManualClock`] instead of sleeping.

mod clock;
mod config;
mod failure;
mod logs;
mod page;
mod service_map;
mod store;
mod summary;
mod trace;

pub use clock::{Clock, ManualClock, SystemClock};
pub use config::StoreConfig;
pub use failure::{FailedIn, TraceStatus, classify, trace_exception};
pub use logs::LogFilter;
pub use page::{Page, PageRequest};
pub use service_map::{NodeKind, ServiceEdge, ServiceMap, ServiceNode, ServiceStats, TimeWindow};
pub use store::{Removed, Store, StoreStats};
pub use summary::{MATCHED_SPANS_MAX, TraceFilter, TraceSummary};
pub use trace::TraceSnapshot;
