//! The performance budgets of loctel (design 10.3, Milestone 5).
//!
//! [`budget`] is the table of them (what each measures and the limit it must
//! stay within), [`Report`](budget::Report) judges measurements against it and
//! says what the exit code of the check is, and the scenarios measure: [`logs`]
//! (`rss_10m`, `logs_count`, `logs_at`, `histogram`, `logql_first_page`),
//! [`traces`] (`trace_open_hot`, `trace_open_cold`, `traceql_first_page`),
//! [`ingest`], [`startup`] (`startup_headless`) and [`frame`] (`first_frame`).
//! They build their data in this process through the store (`ingest` sends it
//! over OTLP to an engine of its own) with the bulk generators of
//! `loctel-meshgen`. [`run::measure`] runs the ones asked for, and `main` is the
//! command line that `cargo xtask perf` builds in release and runs.
//!
//! `docs/areas/testing.md` ("Performance budgets") has the table and the rule
//! about changing it, and `.claude/skills/perf-budgets` the way to run one,
//! read the table and profile a breach.

pub mod budget;
pub mod frame;
pub mod ingest;
pub mod load;
pub mod logs;
mod progress;
mod rss;
pub mod run;
pub mod scale;
pub mod startup;
pub mod stats;
pub mod traces;
pub mod wants;
