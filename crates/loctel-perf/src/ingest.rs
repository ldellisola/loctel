//! The `ingest` budget: loctel under the load the design names (50,000 spans
//! plus logs a second, with a discard rule on, without dropping anything or
//! stalling), sent the way exporters send it.
//!
//! A real engine runs in this process, spilling into a run directory of its own
//! with one discard rule enabled (one that matches nothing in the data, so every
//! batch is judged and none is cut). Two senders, each a keep-alive HTTP/1.1
//! connection, post as fast as the engine answers, alternating a batch of 1,000
//! spans and a batch of 1,000 logs (OTLP protobuf, no compression) for the whole
//! run. They are closed loops on purpose: what is measured is what loctel
//! sustains, and the budget is a floor under it. The rate is the items the engine
//! accepted over the time from the first request to the last answer, the latency
//! is of every request from sending it to reading its answer, and "dropped" is
//! every item that was sent and is not in the store's count of what it accepted
//! (rejected in an answer, failed in transit, or cut by a rule).
//!
//! The batches are made and encoded on a thread of their own, ahead of the
//! senders, so the generator is not what is measured.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result, bail};
use loctel_engine::{
    DropScope, Engine, EngineConfig, EngineHandle, ListenerState, RuleLanguage, RuleSpec, Settings,
};
use loctel_meshgen::{BULK_BATCH, bulk_logs, bulk_traces};
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceResponse;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceResponse;
use prost::Message;
use tokio::sync::{Mutex, mpsc};

use crate::budget::{INGEST_DROPPED, INGEST_P99, INGEST_RATE, Measurement};
use crate::load::SEED;
use crate::progress::say;
use crate::scale::Scale;
use crate::stats::percentile;

/// Requests of the senders in flight at once: two exporters.
const SENDERS: usize = 2;

/// Batches encoded ahead of the senders.
const AHEAD: usize = 16;

/// One pair of requests: the encoded bodies of a batch of spans and a batch of
/// logs.
struct Pair {
    spans: Vec<u8>,
    logs: Vec<u8>,
}

/// What the senders found out.
#[derive(Default)]
struct Sent {
    /// Items in the requests that were sent.
    items: u64,
    /// Items in the requests that failed or that the answer says were rejected.
    failed: u64,
    /// The time each request took, in milliseconds.
    latencies: Vec<f64>,
}

fn now_unix_nano() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_nanos()).unwrap_or(u64::MAX)
        })
}

/// The rule that is on while loctel is loaded: it matches the health checks of
/// the fake mesh, which the bulk data has none of.
fn rule() -> RuleSpec {
    RuleSpec {
        name: "Health checks".to_owned(),
        enabled: true,
        language: RuleLanguage::TraceQl,
        query: r#"{ span.url.path = "/health" }"#.to_owned(),
        scope: DropScope::Trace,
    }
}

fn http_address(engine: &EngineHandle) -> Result<SocketAddr> {
    engine
        .status()
        .listeners
        .iter()
        .find(|listener| listener.name == "otlp-http")
        .and_then(|listener| match &listener.state {
            ListenerState::Listening(address) => Some(*address),
            ListenerState::Failed(_) => None,
        })
        .context("the OTLP HTTP receiver is not listening")
}

/// Makes and encodes pairs until `stop` is set, and hands them to `queue`.
fn produce(queue: mpsc::Sender<Pair>, stop: Arc<AtomicBool>) {
    let start = now_unix_nano();
    // Far more than a run sends: the iterators make a batch when asked.
    let mut spans = bulk_traces(usize::MAX / 2, SEED).starting_at(start);
    let mut logs = bulk_logs(usize::MAX / 2, SEED).starting_at(start);
    while !stop.load(Ordering::Relaxed) {
        let (Some(spans), Some(logs)) = (spans.next(), logs.next()) else {
            break;
        };
        let pair = Pair {
            spans: spans.traces.expect("a traces batch").encode_to_vec(),
            logs: logs.logs.expect("a logs batch").encode_to_vec(),
        };
        if queue.blocking_send(pair).is_err() {
            break;
        }
    }
}

/// Posts `body` to `url`, and says whether the receiver took every item: a 2xx
/// whose answer rejects nothing.
async fn post(client: &reqwest::Client, url: &str, body: Vec<u8>, logs: bool) -> bool {
    let Ok(response) = client
        .post(url)
        .header("content-type", "application/x-protobuf")
        .body(body)
        .send()
        .await
    else {
        return false;
    };
    let ok = response.status().is_success();
    let Ok(bytes) = response.bytes().await else {
        return false;
    };
    if !ok {
        return false;
    }
    let rejected = if logs {
        ExportLogsServiceResponse::decode(bytes.as_ref())
            .ok()
            .and_then(|answer| answer.partial_success)
            .map_or(0, |partial| partial.rejected_log_records)
    } else {
        ExportTraceServiceResponse::decode(bytes.as_ref())
            .ok()
            .and_then(|answer| answer.partial_success)
            .map_or(0, |partial| partial.rejected_spans)
    };
    rejected == 0
}

async fn sender(
    base: String,
    queue: Arc<Mutex<mpsc::Receiver<Pair>>>,
    deadline: Instant,
    batches: Arc<AtomicU64>,
) -> Sent {
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("an HTTP client builds");
    let mut sent = Sent::default();
    while Instant::now() < deadline {
        let Some(pair) = queue.lock().await.recv().await else {
            break;
        };
        for (path, body, logs) in [
            ("/v1/traces", pair.spans, false),
            ("/v1/logs", pair.logs, true),
        ] {
            let started = Instant::now();
            let ok = post(&client, &format!("{base}{path}"), body, logs).await;
            sent.latencies
                .push(started.elapsed().as_secs_f64() * 1_000.0);
            sent.items += BULK_BATCH as u64;
            if !ok {
                sent.failed += BULK_BATCH as u64;
            }
        }
        batches.fetch_add(1, Ordering::Relaxed);
    }
    sent
}

pub fn run(scale: &Scale) -> Result<Vec<Measurement>> {
    let dir = tempfile::tempdir()?;
    let engine = Engine::start(EngineConfig {
        settings: Settings {
            otlp_grpc_port: 0,
            otlp_http_port: 0,
            mcp_port: 0,
            ..Settings::default()
        },
        spill_base: Some(dir.path().join("spill")),
        rules_path: Some(dir.path().join("rules.toml")),
        ..EngineConfig::default()
    })?;
    engine.set_rules(vec![rule()])?;
    let base = format!("http://{}", http_address(&engine)?);
    say(format!(
        "ingest: sending to {base} for {} s",
        scale.ingest.as_secs_f64()
    ));

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(SENDERS)
        .enable_all()
        .build()?;
    let stop = Arc::new(AtomicBool::new(false));
    let (queue, pairs) = mpsc::channel::<Pair>(AHEAD);
    let producer = {
        let stop = Arc::clone(&stop);
        thread::spawn(move || produce(queue, stop))
    };
    let pairs = Arc::new(Mutex::new(pairs));
    let batches = Arc::new(AtomicU64::new(0));

    let started = Instant::now();
    let deadline = started + scale.ingest;
    let senders: Vec<_> = (0..SENDERS)
        .map(|_| {
            runtime.spawn(sender(
                base.clone(),
                Arc::clone(&pairs),
                deadline,
                Arc::clone(&batches),
            ))
        })
        .collect();
    let mut sent = Sent::default();
    for handle in senders {
        let one = runtime.block_on(handle).context("a sender panicked")?;
        sent.items += one.items;
        sent.failed += one.failed;
        sent.latencies.extend(one.latencies);
    }
    let elapsed = started.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    drop(pairs);
    let _ = producer.join();
    drop(runtime);

    let status = engine.status();
    let accepted = status.stats.spans_received + status.stats.logs_received;
    let cut_by_rules = status.dropped_spans + status.dropped_logs;
    let dropped = sent.items.saturating_sub(accepted);
    if sent.items == 0 {
        bail!("the senders sent nothing in {elapsed:.1} s");
    }
    let rate = accepted as f64 / elapsed;
    let p99 = percentile(&mut sent.latencies, 99.0);
    say(format!(
        "ingest: {} requests, {accepted} items accepted of {} sent in {elapsed:.1} s: \
         {rate:.0} items/s, p99 {p99:.1} ms, {} failed, {cut_by_rules} cut by the rule, \
         {} MB on disk",
        sent.latencies.len(),
        sent.items,
        sent.failed,
        status.disk_bytes / 1_000_000
    ));
    engine.shutdown();

    Ok(vec![
        Measurement::value(&INGEST_RATE, rate),
        Measurement::value(&INGEST_DROPPED, dropped as f64),
        Measurement::value(&INGEST_P99, p99),
    ])
}
