//! Where converted telemetry goes, and the export logic both transports share.

use loctel_model::{LogRecord, Span};
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsPartialSuccess, ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTracePartialSuccess, ExportTraceServiceRequest, ExportTraceServiceResponse,
};

use crate::convert::{Converted, logs_from_request, spans_from_request};

/// The receiver of everything the servers accept. Implemented by the engine
/// (pause and store) and by test recorders.
///
/// Both methods are called from async request handlers, so they must return
/// quickly and must not block on I/O. An empty batch is never delivered.
pub trait Sink: Send + Sync + 'static {
    fn spans(&self, spans: Vec<Span>);
    fn logs(&self, logs: Vec<LogRecord>);
}

/// Converts, delivers the valid spans and builds the response, with
/// `partial_success` set when anything was rejected. Shared by gRPC and HTTP.
pub(crate) fn export_traces(
    sink: &dyn Sink,
    request: ExportTraceServiceRequest,
) -> ExportTraceServiceResponse {
    let Converted {
        items,
        rejected,
        error_message,
    } = spans_from_request(request);
    if !items.is_empty() {
        sink.spans(items);
    }
    ExportTraceServiceResponse {
        partial_success: partial(rejected, error_message).map(|(rejected, error_message)| {
            ExportTracePartialSuccess {
                rejected_spans: rejected,
                error_message,
            }
        }),
    }
}

/// The logs counterpart of [`export_traces`].
pub(crate) fn export_logs(
    sink: &dyn Sink,
    request: ExportLogsServiceRequest,
) -> ExportLogsServiceResponse {
    let Converted {
        items,
        rejected,
        error_message,
    } = logs_from_request(request);
    if !items.is_empty() {
        sink.logs(items);
    }
    ExportLogsServiceResponse {
        partial_success: partial(rejected, error_message).map(|(rejected, error_message)| {
            ExportLogsPartialSuccess {
                rejected_log_records: rejected,
                error_message,
            }
        }),
    }
}

/// `Some((rejected, message))` when the response must carry a
/// `partial_success`: anything was rejected, or there is something to say.
fn partial(rejected: u64, error_message: Option<String>) -> Option<(i64, String)> {
    if rejected == 0 && error_message.is_none() {
        return None;
    }
    Some((
        i64::try_from(rejected).unwrap_or(i64::MAX),
        error_message.unwrap_or_default(),
    ))
}
