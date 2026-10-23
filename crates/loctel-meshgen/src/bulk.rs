//! Bulk data: millions of logs and spans, streamed a batch at a time without
//! ever holding them all (Milestone 5). The performance budgets
//! (`cargo xtask perf`) and the Logs list at ten million lines are built on
//! them. Unlike the scenarios, nothing here is fixed content: the mix is what
//! the budgets' queries need, and a change to it moves their numbers.
//!
//! - [`bulk_logs`] makes logs 20 us apart (50,000 a second, the design's
//!   throughput target) from [`BULK_START_UNIX_NANO`], across the five
//!   services of the mesh. 35% are debug, 50% info, 10% warn and 5% error, a
//!   quarter carry a trace and a span ID, and about 1 in 100 of
//!   `payments-api`'s says `timeout`.
//! - [`bulk_traces`] makes traces of ten spans over four services, one starting
//!   every 200 us (50,000 spans a second), about 1 in 100 of them failed (the
//!   call chain from the leaf that failed up to the root is in error).
//!
//! Both are iterators of [`Batch`]es of 1,000 items (a trace never splits
//! across batches while the batch size is a multiple of ten), deterministic
//! for a seed, and on streams of their own so they never share IDs with the
//! scenarios.

use std::collections::VecDeque;
use std::time::Duration;

use loctel_model::{SpanId, TraceId};
use opentelemetry_proto::tonic::logs::v1::LogRecord;
use opentelemetry_proto::tonic::trace::v1::Span;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind;

use crate::scenario::{
    Batch, Level, Rand, SERVICES, int_kv, log, log_request, set_error, span, str_kv, stream,
    trace_request,
};

/// When the first bulk item happens: 2027-01-15T08:00:00Z, the base the
/// screenshot tests use too. `starting_at` moves it.
pub const BULK_START_UNIX_NANO: u64 = 1_800_000_000_000_000_000;

/// Items per batch unless `in_batches_of` says otherwise: what the ingest
/// budget sends, and about what an exporter flushes at once.
pub const BULK_BATCH: usize = 1_000;

/// Logs are this far apart: 50,000 a second.
const LOG_SPACING_NANOS: u64 = 20_000;

/// Spans in a bulk trace (the last trace is cut short when the count is not a
/// multiple).
const SPANS_PER_TRACE: usize = 10;

/// Traces start this far apart: ten spans every 200 us is 50,000 spans a
/// second.
const TRACE_SPACING_NANOS: u64 = 200_000;

/// The chance, out of 100, that a trace fails.
const FAILING_TRACES_PER_100: u64 = 1;

/// The chance, out of 100, that a log of `payments-api` is a timeout.
const TIMEOUTS_PER_100: u64 = 1;

/// `n` logs, streamed as batches. See the module's description.
pub fn bulk_logs(n: usize, seed: u64) -> BulkLogs {
    BulkLogs {
        rand: Rand::new(seed, stream::BULK_LOGS),
        seed,
        remaining: n,
        index: 0,
        start: BULK_START_UNIX_NANO,
        batch: BULK_BATCH,
    }
}

/// `n_spans` spans in traces of ten, streamed as batches. See the module's
/// description.
pub fn bulk_traces(n_spans: usize, seed: u64) -> BulkTraces {
    BulkTraces {
        rand: Rand::new(seed, stream::BULK_TRACES),
        remaining: n_spans,
        pending: VecDeque::new(),
        trace_index: 0,
        start: BULK_START_UNIX_NANO,
        batch: BULK_BATCH,
    }
}

/// The batches of [`bulk_logs`].
#[derive(Clone, Debug)]
pub struct BulkLogs {
    rand: Rand,
    seed: u64,
    /// Logs not made yet.
    remaining: usize,
    /// The number of the next log: its time is `start` plus this many
    /// spacings.
    index: u64,
    start: u64,
    batch: usize,
}

impl BulkLogs {
    /// The time of the first log, in nanoseconds since the Unix epoch, instead
    /// of [`BULK_START_UNIX_NANO`].
    pub fn starting_at(mut self, unix_nano: u64) -> Self {
        self.start = unix_nano;
        self
    }

    /// Logs per batch instead of [`BULK_BATCH`] (at least 1).
    pub fn in_batches_of(mut self, size: usize) -> Self {
        self.batch = size.max(1);
        self
    }

    fn next_log(&mut self) -> (&'static str, LogRecord) {
        let i = self.index;
        self.index += 1;
        let service = SERVICES[self.rand.below(SERVICES.len() as u64) as usize];
        let timeout = service == "payments-api" && self.rand.below(100) < TIMEOUTS_PER_100;
        let level = if timeout {
            if self.rand.below(2) == 0 {
                Level::Warn
            } else {
                Level::Error
            }
        } else {
            match self.rand.below(100) {
                0..35 => Level::Debug,
                35..85 => Level::Info,
                85..95 => Level::Warn,
                _ => Level::Error,
            }
        };
        let body = if timeout {
            let ms = 2_000 + self.rand.below(4_000);
            let attempt = 1 + self.rand.below(3);
            format!("upstream timeout calling card-gateway after {ms} ms (attempt {attempt})")
        } else {
            body(&mut self.rand, level)
        };
        let context = (self.rand.below(4) == 0).then(|| context_ids(self.seed, i));
        let time = self
            .start
            .saturating_add(i.saturating_mul(LOG_SPACING_NANOS));
        (service, log(time, level, &body, context))
    }
}

impl Iterator for BulkLogs {
    type Item = Batch;

    fn next(&mut self) -> Option<Batch> {
        let take = self.remaining.min(self.batch);
        if take == 0 {
            return None;
        }
        let items: Vec<_> = (0..take).map(|_| self.next_log()).collect();
        self.remaining -= take;
        Some(Batch {
            traces: None,
            logs: Some(log_request(items)),
            delay_before: Duration::ZERO,
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let batches = self.remaining.div_ceil(self.batch);
        (batches, Some(batches))
    }
}

impl ExactSizeIterator for BulkLogs {}

/// What a log of `level` says, with a number in it so that bodies differ.
fn body(rand: &mut Rand, level: Level) -> String {
    let n = rand.below(100_000);
    match (level, rand.below(4)) {
        (Level::Debug, 0) => format!("cache lookup key=user:{n} hit={}", !n.is_multiple_of(3)),
        (Level::Debug, 1) => format!("connection pool: {} idle, {} in use", n % 8, n % 5),
        (Level::Debug, 2) => format!("serialized response in {} us", n % 900 + 20),
        (Level::Debug, _) => format!("retry budget {}% remaining", n % 100),
        (Level::Info, 0) => format!("handled request id={n} in {} ms", n % 250 + 1),
        (Level::Info, 1) => format!("processed message id={n}"),
        (Level::Info, 2) => format!("flushed {} records", n % 500 + 1),
        (Level::Info, _) => format!("scheduled job job-{} finished", n % 40),
        (Level::Warn, 0) => format!("slow query took {} ms table=orders", n % 2_000 + 200),
        (Level::Warn, 1) => format!("retrying request id={n} (attempt 2)"),
        (Level::Warn, 2) => format!("cache miss rate high: {}%", n % 60 + 40),
        (Level::Warn, _) => format!("queue depth {} above the soft limit", n % 900 + 100),
        (Level::Error, 0) => format!("failed to process request id={n}: connection reset"),
        (Level::Error, 1) => format!("unhandled exception in handler id={n}"),
        (Level::Error, 2) => format!("rejected message id={n}: invalid payload"),
        (Level::Error, _) => format!("write failed for key=user:{n}: deadline exceeded"),
    }
}

/// splitmix64: a good mix of one number, for IDs that look random but need no
/// generator state.
fn mix(mut x: u64) -> u64 {
    x = x.wrapping_add(0x9e37_79b9_7f4a_7c15);
    x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// The trace and span a log is linked to: sixteen consecutive logs share a
/// trace, each has a span of its own, so that the chunks' trace lists have
/// some weight without every log being a trace of its own. Never all zero.
fn context_ids(seed: u64, i: u64) -> (TraceId, SpanId) {
    let high = mix(seed ^ mix(i / 16));
    let low = mix(high ^ 0xa076_1d64_78bd_642f);
    let mut trace = [0u8; 16];
    trace[..8].copy_from_slice(&high.to_be_bytes());
    trace[8..].copy_from_slice(&(low | 1).to_be_bytes());
    let span = mix(seed.rotate_left(17) ^ i).to_be_bytes();
    let mut span_id = span;
    span_id[7] |= 1;
    (TraceId(trace), SpanId(span_id))
}

/// One span of the shape of a bulk trace.
struct Node {
    /// The index of the parent in [`SHAPE`]; `None` for the root.
    parent: Option<usize>,
    service: &'static str,
    kind: SpanKind,
    name: &'static str,
    /// Offsets from the start of the trace, in microseconds.
    start_us: u64,
    duration_us: u64,
    /// The string attribute that says what it talks to, if it is a client call.
    peer: Option<(&'static str, &'static str)>,
}

/// The ten spans of a bulk trace: a request into `web-bff`, through
/// `orders-api`, to `inventory-api` (and its cache) and `payments-api` (and its
/// database). Every span lies inside its parent.
const SHAPE: [Node; SPANS_PER_TRACE] = [
    Node {
        parent: None,
        service: "web-bff",
        kind: SpanKind::Server,
        name: "GET /api/items",
        start_us: 0,
        duration_us: 40_000,
        peer: None,
    },
    Node {
        parent: Some(0),
        service: "web-bff",
        kind: SpanKind::Client,
        name: "GET",
        start_us: 1_000,
        duration_us: 38_000,
        peer: Some(("server.address", "orders-api")),
    },
    Node {
        parent: Some(1),
        service: "orders-api",
        kind: SpanKind::Server,
        name: "GET /items",
        start_us: 2_000,
        duration_us: 36_000,
        peer: None,
    },
    Node {
        parent: Some(2),
        service: "orders-api",
        kind: SpanKind::Client,
        name: "SELECT items",
        start_us: 3_000,
        duration_us: 6_000,
        peer: Some(("db.system.name", "postgresql")),
    },
    Node {
        parent: Some(2),
        service: "orders-api",
        kind: SpanKind::Client,
        name: "GET",
        start_us: 10_000,
        duration_us: 12_000,
        peer: Some(("server.address", "inventory-api")),
    },
    Node {
        parent: Some(4),
        service: "inventory-api",
        kind: SpanKind::Server,
        name: "GET /stock/{sku}",
        start_us: 11_000,
        duration_us: 10_000,
        peer: None,
    },
    Node {
        parent: Some(5),
        service: "inventory-api",
        kind: SpanKind::Client,
        name: "GET",
        start_us: 12_000,
        duration_us: 2_000,
        peer: Some(("db.system.name", "redis")),
    },
    Node {
        parent: Some(2),
        service: "orders-api",
        kind: SpanKind::Client,
        name: "POST",
        start_us: 23_000,
        duration_us: 12_000,
        peer: Some(("server.address", "payments-api")),
    },
    Node {
        parent: Some(7),
        service: "payments-api",
        kind: SpanKind::Server,
        name: "POST /quote",
        start_us: 24_000,
        duration_us: 10_000,
        peer: None,
    },
    Node {
        parent: Some(8),
        service: "payments-api",
        kind: SpanKind::Client,
        name: "SELECT prices",
        start_us: 25_000,
        duration_us: 5_000,
        peer: Some(("db.system.name", "postgresql")),
    },
];

/// The spans that are in error in a failed trace: the leaf that failed and
/// every call above it, up to the root.
const FAILED_CHAIN: [usize; 6] = [9, 8, 7, 2, 1, 0];

/// The batches of [`bulk_traces`].
#[derive(Clone, Debug)]
pub struct BulkTraces {
    rand: Rand,
    /// Spans not made yet (those in `pending` are made).
    remaining: usize,
    /// Spans made but not yet in a batch.
    pending: VecDeque<(&'static str, Span)>,
    trace_index: u64,
    start: u64,
    batch: usize,
}

impl BulkTraces {
    /// The start of the first trace, in nanoseconds since the Unix epoch,
    /// instead of [`BULK_START_UNIX_NANO`].
    pub fn starting_at(mut self, unix_nano: u64) -> Self {
        self.start = unix_nano;
        self
    }

    /// Spans per batch instead of [`BULK_BATCH`] (at least 1).
    pub fn in_batches_of(mut self, size: usize) -> Self {
        self.batch = size.max(1);
        self
    }

    /// Makes the next trace (or its first spans, when fewer are left) and puts
    /// its spans in `pending`.
    fn make_trace(&mut self) {
        let trace = self.rand.trace_id();
        let ids: Vec<SpanId> = (0..SPANS_PER_TRACE).map(|_| self.rand.span_id()).collect();
        let failed = self.rand.below(100) < FAILING_TRACES_PER_100;
        // The whole trace is stretched by 1 to 2 times, so that durations
        // differ and a parent still holds its children exactly: the offsets are
        // microseconds, and a thousand times them divides by 8.
        let stretch = 8 + self.rand.below(8);
        let begin = self
            .start
            .saturating_add(self.trace_index.saturating_mul(TRACE_SPACING_NANOS));
        self.trace_index += 1;

        let made = self.remaining.min(SPANS_PER_TRACE);
        self.remaining -= made;
        for (i, node) in SHAPE.iter().enumerate().take(made) {
            let from = begin + node.start_us * 1_000 * stretch / 8;
            let to = from + node.duration_us * 1_000 * stretch / 8;
            let mut built = span(
                trace,
                ids[i],
                node.parent.map(|parent| ids[parent]),
                node.name,
                node.kind,
                from,
                to,
            );
            let in_error = failed && FAILED_CHAIN.contains(&i);
            let mut attributes = Vec::with_capacity(3);
            if let Some((key, value)) = node.peer {
                attributes.push(str_kv(key, value));
            }
            if node.kind == SpanKind::Server {
                attributes.push(str_kv(
                    "http.request.method",
                    if node.name.starts_with("POST") {
                        "POST"
                    } else {
                        "GET"
                    },
                ));
                attributes.push(int_kv(
                    "http.response.status_code",
                    if in_error { 500 } else { 200 },
                ));
            }
            built.attributes = attributes;
            if in_error {
                set_error(&mut built, "upstream request failed");
            }
            self.pending.push_back((node.service, built));
        }
    }
}

impl Iterator for BulkTraces {
    type Item = Batch;

    fn next(&mut self) -> Option<Batch> {
        let take = self.batch.min(self.pending.len() + self.remaining);
        if take == 0 {
            return None;
        }
        while self.pending.len() < take {
            self.make_trace();
        }
        let items: Vec<_> = self.pending.drain(..take).collect();
        Some(Batch {
            traces: Some(trace_request(items)),
            logs: None,
            delay_before: Duration::ZERO,
        })
    }
}
