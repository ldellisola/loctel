//! Delivering generated batches to a receiver.

use std::io::Write as _;
use std::time::Duration;

use anyhow::{Context, Result, bail, ensure};
use flate2::Compression;
use flate2::write::GzEncoder;
use opentelemetry_proto::tonic::collector::logs::v1::logs_service_client::LogsServiceClient;
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_client::TraceServiceClient;
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use prost::Message;
use reqwest::header::{CONTENT_ENCODING, CONTENT_TYPE};
use serde_json::Value;
use tonic::transport::{Channel, Endpoint};

use crate::scenario::Generated;

const PROTOBUF: &str = "application/x-protobuf";
const JSON: &str = "application/json";

/// HTTP bodies at least this big are gzipped (and say so), which keeps the
/// compressed path in use for big scenarios. Small ones go out plain, like
/// most exporters send them.
const GZIP_THRESHOLD: usize = 64 * 1024;

/// How long one request may take before it counts as failed.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Where and how to send. Each holds the endpoint's base URL: for gRPC the
/// server address (`http://127.0.0.1:4317`), for HTTP the base the OTLP
/// paths hang off (`http://127.0.0.1:4318`, giving `/v1/traces` and
/// `/v1/logs`). A missing `http://` is added, and a trailing `/` ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Transport {
    Grpc(String),
    HttpProtobuf(String),
    HttpJson(String),
}

/// What was sent: spans and log records, counted from the requests.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SendReport {
    pub spans: usize,
    pub logs: usize,
}

/// Sends every batch in order. Before each batch it sleeps the batch's
/// `delay_before` times `time_scale` (0 sends everything at once, 1 keeps the
/// scenario's real pauses). Fails on the first batch that cannot be delivered,
/// and also when the receiver accepts the request but reports rejected items
/// (`partial_success`), because generated data is always valid.
pub async fn send(g: &Generated, t: &Transport, time_scale: f64) -> Result<SendReport> {
    ensure!(
        time_scale.is_finite() && time_scale >= 0.0,
        "time_scale must be 0 or more, got {time_scale}"
    );
    let mut sender = Sender::connect(t).await?;
    let mut report = SendReport::default();
    for batch in &g.batches {
        let delay = Duration::try_from_secs_f64(batch.delay_before.as_secs_f64() * time_scale)
            .context("the scaled delay is too long")?;
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        if let Some(request) = &batch.traces {
            sender.traces(request).await?;
            report.spans += span_count(request);
        }
        if let Some(request) = &batch.logs {
            sender.logs(request).await?;
            report.logs += log_count(request);
        }
    }
    Ok(report)
}

fn span_count(request: &ExportTraceServiceRequest) -> usize {
    request
        .resource_spans
        .iter()
        .flat_map(|r| &r.scope_spans)
        .map(|s| s.spans.len())
        .sum()
}

fn log_count(request: &ExportLogsServiceRequest) -> usize {
    request
        .resource_logs
        .iter()
        .flat_map(|r| &r.scope_logs)
        .map(|s| s.log_records.len())
        .sum()
}

/// `http://` added when the URL has no scheme, no trailing slash.
fn base_url(endpoint: &str) -> String {
    let endpoint = endpoint.trim().trim_end_matches('/');
    if endpoint.contains("://") {
        endpoint.to_owned()
    } else {
        format!("http://{endpoint}")
    }
}

/// The two gRPC services over one connection.
struct GrpcClients {
    traces: TraceServiceClient<Channel>,
    logs: LogsServiceClient<Channel>,
}

/// An open connection to the receiver.
enum Sender {
    Grpc(Box<GrpcClients>),
    Http {
        client: reqwest::Client,
        base: String,
        json: bool,
    },
}

impl Sender {
    async fn connect(t: &Transport) -> Result<Sender> {
        match t {
            Transport::Grpc(endpoint) => {
                let url = base_url(endpoint);
                let channel = Endpoint::from_shared(url.clone())
                    .with_context(|| format!("{url} is not a valid endpoint"))?
                    .connect_timeout(Duration::from_secs(10))
                    .timeout(REQUEST_TIMEOUT)
                    .connect()
                    .await
                    .with_context(|| format!("could not connect to {url} over gRPC"))?;
                Ok(Sender::Grpc(Box::new(GrpcClients {
                    traces: TraceServiceClient::new(channel.clone()),
                    logs: LogsServiceClient::new(channel),
                })))
            }
            Transport::HttpProtobuf(endpoint) | Transport::HttpJson(endpoint) => {
                // No proxy: meshgen talks to loctel on this machine.
                let client = reqwest::Client::builder()
                    .no_proxy()
                    .timeout(REQUEST_TIMEOUT)
                    .build()
                    .context("could not build the HTTP client")?;
                Ok(Sender::Http {
                    client,
                    base: base_url(endpoint),
                    json: matches!(t, Transport::HttpJson(_)),
                })
            }
        }
    }

    async fn traces(&mut self, request: &ExportTraceServiceRequest) -> Result<()> {
        let rejected = match self {
            Sender::Grpc(grpc) => {
                let response = grpc
                    .traces
                    .export(request.clone())
                    .await
                    .context("gRPC trace export failed")?
                    .into_inner();
                response
                    .partial_success
                    .map(|p| (p.rejected_spans, p.error_message))
            }
            Sender::Http { json, .. } => {
                let json = *json;
                let body = encode(request, json)?;
                let bytes = self.post("/v1/traces", body).await?;
                if json {
                    rejected_in_json(&bytes, "rejectedSpans")?
                } else {
                    ExportTraceServiceResponse::decode(bytes.as_slice())
                        .context("the trace response is not protobuf")?
                        .partial_success
                        .map(|p| (p.rejected_spans, p.error_message))
                }
            }
        };
        reject_if_partial("spans", rejected)
    }

    async fn logs(&mut self, request: &ExportLogsServiceRequest) -> Result<()> {
        let rejected = match self {
            Sender::Grpc(grpc) => {
                let response = grpc
                    .logs
                    .export(request.clone())
                    .await
                    .context("gRPC logs export failed")?
                    .into_inner();
                response
                    .partial_success
                    .map(|p| (p.rejected_log_records, p.error_message))
            }
            Sender::Http { json, .. } => {
                let json = *json;
                let body = encode(request, json)?;
                let bytes = self.post("/v1/logs", body).await?;
                if json {
                    rejected_in_json(&bytes, "rejectedLogRecords")?
                } else {
                    ExportLogsServiceResponse::decode(bytes.as_slice())
                        .context("the logs response is not protobuf")?
                        .partial_success
                        .map(|p| (p.rejected_log_records, p.error_message))
                }
            }
        };
        reject_if_partial("log records", rejected)
    }

    /// POSTs `body` (already in the transport's format) and returns the
    /// response body. Anything but a 2xx is an error carrying the body.
    async fn post(&self, path: &str, body: Vec<u8>) -> Result<Vec<u8>> {
        let Sender::Http { client, base, json } = self else {
            bail!("post is for HTTP transports only");
        };
        let url = format!("{base}{path}");
        let mut request = client
            .post(&url)
            .header(CONTENT_TYPE, if *json { JSON } else { PROTOBUF });
        let body = if body.len() >= GZIP_THRESHOLD {
            request = request.header(CONTENT_ENCODING, "gzip");
            gzip(&body)?
        } else {
            body
        };
        let response = request
            .body(body)
            .send()
            .await
            .with_context(|| format!("POST {url} failed"))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .with_context(|| format!("could not read the response of POST {url}"))?;
        if !status.is_success() {
            bail!(
                "POST {url} returned {status}: {}",
                String::from_utf8_lossy(&bytes)
            );
        }
        Ok(bytes.to_vec())
    }
}

/// The request as the transport's format: OTLP/JSON or protobuf.
fn encode<T: Message + serde::Serialize>(request: &T, json: bool) -> Result<Vec<u8>> {
    if json {
        serde_json::to_vec(request).context("could not encode the request as JSON")
    } else {
        Ok(request.encode_to_vec())
    }
}

fn gzip(body: &[u8]) -> Result<Vec<u8>> {
    let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(body).context("gzip failed")?;
    encoder.finish().context("gzip failed")
}

/// `partialSuccess.<field>` of an OTLP/JSON response. int64 is a string in
/// OTLP/JSON, but a number is accepted too.
fn rejected_in_json(body: &[u8], field: &str) -> Result<Option<(i64, String)>> {
    let value: Value = serde_json::from_slice(body).context("the response is not JSON")?;
    let Some(partial) = value.get("partialSuccess") else {
        return Ok(None);
    };
    let rejected = match partial.get(field) {
        Some(Value::String(s)) => s.parse().context("rejected count is not a number")?,
        Some(Value::Number(n)) => n.as_i64().context("rejected count is not an integer")?,
        _ => 0,
    };
    let message = partial
        .get("errorMessage")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    Ok(Some((rejected, message)))
}

/// Generated data is valid, so a receiver that rejects some of it is
/// reporting a bug somewhere. Say so instead of counting them as sent.
fn reject_if_partial(what: &str, rejected: Option<(i64, String)>) -> Result<()> {
    match rejected {
        Some((rejected, message)) if rejected > 0 => {
            bail!("the receiver rejected {rejected} {what}: {message}")
        }
        _ => Ok(()),
    }
}
