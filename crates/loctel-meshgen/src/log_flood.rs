//! `LogFlood`: a lot of ordinary logs with no trace context, for the log
//! store and its search. INF and DBG records spread across every service.

use std::time::Duration;

use opentelemetry_proto::tonic::logs::v1::LogRecord;

use crate::scenario::{Batch, Generated, Level, Rand, SERVICES, log, log_request, stream};

/// Records per batch.
const RECORDS_PER_BATCH: usize = 10_000;

/// Records are this far apart (50,000 per second, the design's throughput
/// target). Distinct times keep the store's duplicate check from merging
/// records that happen to say the same thing.
const SPACING_NANOS: u64 = 20_000;

fn message(rand: &mut Rand, level: Level) -> String {
    let n = rand.below(100_000);
    match (level, rand.below(4)) {
        (Level::Debug, 0) => format!("cache lookup key=user:{n} hit={}", !n.is_multiple_of(3)),
        (Level::Debug, 1) => format!("connection pool: {} idle, {} in use", n % 8, n % 5),
        (Level::Debug, 2) => format!("serialized response in {} us", n % 900 + 20),
        (Level::Debug, _) => format!("retry budget {}% remaining", n % 100),
        (_, 0) => format!("handled request id={n} in {} ms", n % 250 + 1),
        (_, 1) => format!("processed message id={n}"),
        (_, 2) => format!("flushed {} records", n % 500 + 1),
        (_, _) => format!("scheduled job job-{} finished", n % 40),
    }
}

pub(crate) fn generate(records: usize, seed: u64, base: u64) -> Generated {
    let mut rand = Rand::new(seed, stream::LOG_FLOOD);
    let all: Vec<(&'static str, LogRecord)> = (0..records)
        .map(|i| {
            let service = SERVICES[rand.below(SERVICES.len() as u64) as usize];
            // About 4 in 10 are debug.
            let level = if rand.below(10) < 4 {
                Level::Debug
            } else {
                Level::Info
            };
            let body = message(&mut rand, level);
            let time = base.saturating_add(i as u64 * SPACING_NANOS);
            (service, log(time, level, &body, None))
        })
        .collect();

    let batches = all
        .chunks(RECORDS_PER_BATCH)
        .map(|chunk| Batch {
            traces: None,
            logs: Some(log_request(chunk.to_vec())),
            delay_before: Duration::ZERO,
        })
        .collect();
    Generated {
        batches,
        trace_ids: Vec::new(),
    }
}
