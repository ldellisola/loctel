//! The OTLP/HTTP receiver: `POST /v1/traces`, `/v1/logs` and `/v1/metrics`,
//! protobuf or JSON, optionally gzip-compressed.

use std::future::Future;
use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use opentelemetry_proto::tonic::collector::logs::v1::ExportLogsServiceRequest;
use opentelemetry_proto::tonic::collector::trace::v1::ExportTraceServiceRequest;
use prost::Message;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower_http::decompression::RequestDecompressionLayer;

use crate::sink::{Sink, export_logs, export_traces};

const PROTOBUF: &str = "application/x-protobuf";
const JSON: &str = "application/json";

/// The two body encodings OTLP/HTTP defines. A reply always uses the
/// encoding of its request.
#[derive(Clone, Copy)]
enum Format {
    Protobuf,
    Json,
}

impl Format {
    /// From the `Content-Type` header, ignoring case and parameters such as
    /// `; charset=utf-8`. `None` for anything else, including no header.
    fn of(headers: &HeaderMap) -> Option<Format> {
        let value = headers.get(CONTENT_TYPE)?.to_str().ok()?;
        let media_type = value.split(';').next()?.trim();
        if media_type.eq_ignore_ascii_case(PROTOBUF) {
            Some(Format::Protobuf)
        } else if media_type.eq_ignore_ascii_case(JSON) {
            Some(Format::Json)
        } else {
            None
        }
    }

    fn content_type(self) -> &'static str {
        match self {
            Format::Protobuf => PROTOBUF,
            Format::Json => JSON,
        }
    }
}

/// The routes. Gzip is inflated first, then the body is read with a limit of
/// `max_message_bytes` on the *inflated* size: axum's own default is 2 MB,
/// which real batches exceed, and a gzip bomb must not get past the limit.
fn router(sink: Arc<dyn Sink>, max_message_bytes: usize) -> Router {
    Router::new()
        .route("/v1/traces", post(traces))
        .route("/v1/logs", post(logs))
        .route("/v1/metrics", post(metrics))
        .with_state(sink)
        .layer(DefaultBodyLimit::max(max_message_bytes))
        // gzip is the only decoder asked for (the `decompression-gzip` feature);
        // an encoding it cannot decode is a 415.
        .layer(RequestDecompressionLayer::new())
}

/// Serves OTLP/HTTP on `listener` until `shutdown` completes.
pub(crate) async fn serve(
    listener: TcpListener,
    sink: Arc<dyn Sink>,
    max_message_bytes: usize,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> std::io::Result<()> {
    axum::serve(listener, router(sink, max_message_bytes))
        .with_graceful_shutdown(shutdown)
        .await
}

// --- handlers ---------------------------------------------------------------

async fn traces(
    State(sink): State<Arc<dyn Sink>>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Rejected> {
    let (format, request) = decode::<ExportTraceServiceRequest>(&headers, body)?;
    let response = export_traces(&*sink, request);
    let partial = response
        .partial_success
        .as_ref()
        .map(|p| ("rejectedSpans", p.rejected_spans, p.error_message.as_str()));
    Ok(reply(format, response.encode_to_vec(), partial))
}

async fn logs(
    State(sink): State<Arc<dyn Sink>>,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<Response, Rejected> {
    let (format, request) = decode::<ExportLogsServiceRequest>(&headers, body)?;
    let response = export_logs(&*sink, request);
    let partial = response.partial_success.as_ref().map(|p| {
        (
            "rejectedLogRecords",
            p.rejected_log_records,
            p.error_message.as_str(),
        )
    });
    Ok(reply(format, response.encode_to_vec(), partial))
}

/// Metrics are accepted and dropped without being decoded, so that exporters
/// never log errors.
async fn metrics(headers: HeaderMap) -> Result<Response, Rejected> {
    let format = Format::of(&headers).ok_or_else(Rejected::unsupported_media_type)?;
    Ok(reply(format, Vec::new(), None))
}

// --- decoding and replying --------------------------------------------------

/// A request that gets an error response instead of an export.
struct Rejected {
    /// The request's format, when it had a usable one: the error body is
    /// written in it. `None` gets a plain-text body.
    format: Option<Format>,
    status: StatusCode,
    message: String,
}

impl Rejected {
    fn unsupported_media_type() -> Self {
        Rejected {
            format: None,
            status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
            message: format!("Content-Type must be {PROTOBUF} or {JSON}"),
        }
    }
}

impl IntoResponse for Rejected {
    fn into_response(self) -> Response {
        match self.format {
            Some(format) => error_reply(format, self.status, &self.message),
            None => (self.status, self.message).into_response(),
        }
    }
}

/// Reads the request in its declared format. Fails with 415 for an unknown
/// content type, with the limit or transfer error (413, 400) for a body that
/// cannot be read, and with 400 for a body that does not decode.
fn decode<T>(
    headers: &HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> Result<(Format, T), Rejected>
where
    T: Message + Default + DeserializeOwned,
{
    let format = Format::of(headers).ok_or_else(Rejected::unsupported_media_type)?;
    let body = body.map_err(|rejection| Rejected {
        format: Some(format),
        status: rejection.status(),
        message: rejection.body_text(),
    })?;
    let decoded = match format {
        Format::Protobuf => T::decode(body).map_err(|e| e.to_string()),
        Format::Json => serde_json::from_slice(&body).map_err(|e| e.to_string()),
    };
    decoded
        .map(|request| (format, request))
        .map_err(|message| Rejected {
            format: Some(format),
            status: StatusCode::BAD_REQUEST,
            message: format!("could not decode the request: {message}"),
        })
}

/// A 200 with the response message. `protobuf` is the encoded response
/// message. For JSON the response is written out so that `int64` is a string
/// and an empty response is `{}`, as OTLP/JSON asks. `partial` is
/// (JSON field name, rejected count, message).
fn reply(format: Format, protobuf: Vec<u8>, partial: Option<(&str, i64, &str)>) -> Response {
    let body = match format {
        Format::Protobuf => protobuf,
        Format::Json => {
            let value = match partial {
                None => json!({}),
                Some((field, rejected, message)) => json!({
                    "partialSuccess": {
                        field: rejected.to_string(),
                        "errorMessage": message,
                    }
                }),
            };
            value.to_string().into_bytes()
        }
    };
    (
        StatusCode::OK,
        [(CONTENT_TYPE, format.content_type())],
        body,
    )
        .into_response()
}

/// `google.rpc.Status`, the error body OTLP/HTTP uses (protobuf-encoded, or
/// as JSON for JSON requests).
#[derive(Clone, PartialEq, Message)]
struct RpcStatus {
    #[prost(int32, tag = "1")]
    code: i32,
    #[prost(string, tag = "2")]
    message: String,
}

fn error_reply(format: Format, status: StatusCode, message: &str) -> Response {
    // gRPC status codes: INVALID_ARGUMENT, RESOURCE_EXHAUSTED, UNKNOWN.
    let code = match status {
        StatusCode::BAD_REQUEST => 3,
        StatusCode::PAYLOAD_TOO_LARGE => 8,
        _ => 2,
    };
    let body = match format {
        Format::Protobuf => RpcStatus {
            code,
            message: message.to_owned(),
        }
        .encode_to_vec(),
        Format::Json => Value::to_string(&json!({ "code": code, "message": message })).into_bytes(),
    };
    (status, [(CONTENT_TYPE, format.content_type())], body).into_response()
}
