//! The engine's end of the receivers: pause, the rate meters and the store.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use loctel_ingest::Sink;
use loctel_model::{LogRecord, Span};
use loctel_store::{Clock, Store};

use crate::rates::RateMeter;

/// Where everything the receivers accept goes. While paused it is dropped,
/// not stored, not counted and not metered, and the receivers go on answering
/// success so exporters neither buffer nor retry (design §4).
pub(crate) struct EngineSink {
    pub(crate) store: Store,
    pub(crate) paused: Arc<AtomicBool>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) span_rate: Arc<RateMeter>,
    pub(crate) log_rate: Arc<RateMeter>,
}

impl EngineSink {
    fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }
}

impl Sink for EngineSink {
    fn spans(&self, spans: Vec<Span>) {
        if self.is_paused() {
            return;
        }
        self.span_rate
            .record(self.clock.now_unix_nano(), spans.len() as u64);
        self.store.ingest_spans(spans);
    }

    fn logs(&self, logs: Vec<LogRecord>) {
        if self.is_paused() {
            return;
        }
        self.log_rate
            .record(self.clock.now_unix_nano(), logs.len() as u64);
        self.store.ingest_logs(logs);
    }
}
