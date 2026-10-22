//! A deterministic fake microservice mesh for loctel's tests.
//!
//! [`generate`] turns a [`Scenario`] into OTLP export requests (traces and
//! logs, in [`Batch`]es with the pauses between them), byte for byte the same
//! for the same seed and base time. [`send`] delivers them to a running
//! receiver over gRPC, HTTP protobuf or HTTP JSON. The `meshgen` binary is
//! the command line for both.
//!
//! [`bulk_logs`] and [`bulk_traces`] stream millions of logs and spans a batch
//! at a time, for the performance budgets and the Logs list at ten million
//! lines.
//!
//! [`Scenario::CheckoutFailure`] is fixed content that tests and screenshots
//! depend on. See `docs/areas/testing.md`.

mod bulk;
pub mod checkout;
mod health;
mod huge;
mod log_flood;
mod scenario;
mod send;

pub use bulk::{BULK_BATCH, BULK_START_UNIX_NANO, BulkLogs, BulkTraces, bulk_logs, bulk_traces};
pub use scenario::{
    APP_SERVICES, Batch, ENVIRONMENT, Generated, SERVICE_VERSION, SERVICES, Scenario, generate,
};
pub use send::{SendReport, Transport, send};
