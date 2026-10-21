//! The engine's end of the receivers: pause, the discard rules, the rate
//! meters and the store.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use loctel_ingest::Sink;
use loctel_model::{LogRecord, Span};
use loctel_store::{Clock, Store};
use parking_lot::Mutex;

use crate::rates::RateMeter;
use crate::rules_watch::Rules;

/// Where everything the receivers accept goes. While paused it is dropped,
/// not judged by the rules, not stored, not counted and not metered, and the
/// receivers go on answering success so exporters neither buffer nor retry
/// (design §4). Otherwise every batch goes through the discard rules on its
/// way to the store (design §7), and only what the rules let through is
/// metered and counted as accepted.
pub(crate) struct EngineSink {
    pub(crate) store: Store,
    pub(crate) paused: Arc<AtomicBool>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) span_rate: Arc<RateMeter>,
    pub(crate) log_rate: Arc<RateMeter>,
    pub(crate) rules: Arc<Rules>,
    /// Spans and logs that passed the rules (and were stored), since launch:
    /// the other side of the dropped fraction.
    pub(crate) accepted: Arc<AtomicU64>,
    /// Held for the whole of a batch, spans or logs: judging it, removing
    /// what it condemns from the store and storing the rest. Batches arrive
    /// on several threads (gRPC and HTTP), and a batch that was judged before
    /// another one dropped a trace must not store its spans after that trace
    /// was removed.
    pub(crate) apply: Mutex<()>,
}

impl EngineSink {
    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// Notes that `count` items were accepted, at the rate meter and in the
    /// total.
    fn note_accepted(&self, rate: &RateMeter, count: usize) {
        if count > 0 {
            let count = count as u64;
            rate.record(self.clock.now_unix_nano(), count);
            self.accepted.fetch_add(count, Ordering::Relaxed);
        }
    }
}

impl Sink for EngineSink {
    fn spans(&self, spans: Vec<Span>) {
        if self.is_paused() {
            return;
        }
        let _batch = self.apply.lock();
        let rules = self.rules.set();
        let verdict = rules.filter_spans(spans, &self.store);
        // What the store holds of the traces and spans just condemned goes
        // before this batch's survivors come in, and is credited to the rules.
        rules.remove_dropped(&verdict, &self.store);
        self.note_accepted(&self.span_rate, verdict.keep.len());
        self.store.ingest_spans(verdict.keep);
    }

    fn logs(&self, logs: Vec<LogRecord>) {
        if self.is_paused() {
            return;
        }
        let _batch = self.apply.lock();
        let keep = self.rules.set().filter_logs(logs);
        self.note_accepted(&self.log_rate, keep.len());
        self.store.ingest_logs(keep);
    }
}
