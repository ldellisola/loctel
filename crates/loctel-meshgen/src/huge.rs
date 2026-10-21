//! `HugeTrace`: one trace with a very large number of spans, for the
//! waterfall and the store's limits.
//!
//! The tree is a complete k-ary tree in breadth-first order: span `i`'s
//! parent is span `(i - 1) / k`. The fan-out `k` starts at 10 and grows only
//! as far as needed to keep the depth at 8 levels or fewer, so 100,000 spans
//! take 6 levels. Services rotate among the four app services by span index.
//! Parents always enclose their children in time.

use std::collections::HashSet;
use std::time::Duration;

use loctel_model::SpanId;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind;

use crate::scenario::{APP_SERVICES, Batch, Generated, Rand, span, stream, trace_request};

/// The deepest tree allowed, counting the root as a level.
const MAX_LEVELS: u32 = 8;

/// The smallest fan-out used.
const MIN_FANOUT: u128 = 10;

/// Spans per batch, about what a real exporter would flush at once, only
/// bigger.
const SPANS_PER_BATCH: usize = 5_000;

const MS: u64 = 1_000_000;

/// How many spans a tree with fan-out `k` holds in [`MAX_LEVELS`] levels.
fn capacity(k: u128) -> u128 {
    (0..MAX_LEVELS).fold(0u128, |sum, level| {
        sum.saturating_add(k.saturating_pow(level))
    })
}

/// The smallest fan-out (at least 10) whose tree is deep enough for `spans`.
fn fanout(spans: usize) -> usize {
    let mut k = MIN_FANOUT;
    while capacity(k) < spans as u128 {
        k += 1;
    }
    k as usize
}

pub(crate) fn generate(spans: usize, seed: u64, base: u64) -> Generated {
    if spans == 0 {
        return Generated {
            batches: Vec::new(),
            trace_ids: Vec::new(),
        };
    }

    let mut rand = Rand::new(seed, stream::HUGE_TRACE);
    let trace = rand.trace_id();
    let mut seen = HashSet::with_capacity(spans);
    let ids: Vec<SpanId> = (0..spans)
        .map(|_| {
            loop {
                let id = rand.span_id();
                if seen.insert(id) {
                    break id;
                }
            }
        })
        .collect();

    // Starts go top-down (children begin just after their parent, siblings
    // staggered), ends bottom-up (a parent outlasts its last child).
    let k = fanout(spans);
    let parent = |i: usize| (i - 1) / k;
    let mut start = vec![0u64; spans];
    let mut end = vec![0u64; spans];
    for i in 0..spans {
        if i > 0 {
            start[i] = start[parent(i)] + MS + ((i - 1) % k) as u64 * (MS / 2);
        }
        end[i] = start[i] + MS + rand.below(4 * MS);
    }
    for i in (1..spans).rev() {
        let p = parent(i);
        end[p] = end[p].max(end[i] + MS);
    }

    let all: Vec<_> = (0..spans)
        .map(|i| {
            let (name, kind, parent) = if i == 0 {
                ("POST /api/bulk".to_owned(), SpanKind::Server, None)
            } else {
                (
                    format!("work item {i}"),
                    SpanKind::Internal,
                    Some(ids[parent(i)]),
                )
            };
            let built = span(
                trace,
                ids[i],
                parent,
                &name,
                kind,
                base.saturating_add(start[i]),
                base.saturating_add(end[i]),
            );
            (APP_SERVICES[i % APP_SERVICES.len()], built)
        })
        .collect();

    let batches = all
        .chunks(SPANS_PER_BATCH)
        .map(|chunk| Batch {
            traces: Some(trace_request(chunk.to_vec())),
            logs: None,
            delay_before: Duration::ZERO,
        })
        .collect();
    Generated {
        batches,
        trace_ids: vec![trace],
    }
}
