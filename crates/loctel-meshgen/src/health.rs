//! `HealthChecks`: the noise a load balancer or orchestrator makes. Every
//! service answers `count` probes, and each probe is a trace of its own (a
//! lone `GET /health` server span) with an INF access log linked to it. This
//! is what the discard rules of Milestone 2 exist to silence.

use opentelemetry_proto::tonic::logs::v1::LogRecord;
use opentelemetry_proto::tonic::trace::v1::Span;
use opentelemetry_proto::tonic::trace::v1::span::SpanKind;

use crate::scenario::{
    Batch, Generated, Level, Rand, SERVICES, at_ms, int_kv, log, log_request, span, str_kv, stream,
    trace_request,
};

/// Probes (one span and one log each) per batch.
const PROBES_PER_BATCH: usize = 1_000;

/// Probes arrive once a second per service, with the services offset by this
/// many milliseconds so they do not line up.
const PROBE_INTERVAL_MS: u64 = 1_000;
const SERVICE_OFFSET_MS: u64 = 200;

pub(crate) fn generate(count: usize, seed: u64, base: u64) -> Generated {
    let mut rand = Rand::new(seed, stream::HEALTH_CHECKS);
    let mut trace_ids = Vec::with_capacity(count * SERVICES.len());
    let mut probes: Vec<(&'static str, Span, LogRecord)> =
        Vec::with_capacity(count * SERVICES.len());

    for i in 0..count {
        for (s, service) in SERVICES.into_iter().enumerate() {
            let trace = rand.trace_id();
            let id = rand.span_id();
            trace_ids.push(trace);

            let start = at_ms(
                base,
                i as u64 * PROBE_INTERVAL_MS + s as u64 * SERVICE_OFFSET_MS,
            );
            // 1 to 4 ms.
            let end = start + 1_000_000 + rand.below(3_000_000);
            let mut probe = span(trace, id, None, "GET /health", SpanKind::Server, start, end);
            probe.attributes = vec![
                str_kv("http.request.method", "GET"),
                str_kv("url.path", "/health"),
                int_kv("http.response.status_code", 200),
            ];
            let access_log = log(end, Level::Info, "GET /health 200", Some((trace, id)));
            probes.push((service, probe, access_log));
        }
    }

    let batches = probes
        .chunks(PROBES_PER_BATCH)
        .map(|chunk| Batch {
            traces: Some(trace_request(
                chunk
                    .iter()
                    .map(|(s, span, _)| (*s, span.clone()))
                    .collect(),
            )),
            logs: Some(log_request(
                chunk.iter().map(|(s, _, log)| (*s, log.clone())).collect(),
            )),
            delay_before: std::time::Duration::ZERO,
        })
        .collect();
    Generated { batches, trace_ids }
}
