# Ingest (`loctel-ingest`)

The OTLP receivers behind loctel. They take traces and logs over gRPC and
HTTP, convert them to `loctel-model` types, and hand them to a `Sink` (the
engine, which feeds the store). Metrics are accepted and dropped. Design
references: §4 (endpoints, reachability, ports, order and lateness, logs,
exceptions, pause) and §10.5 (known gotchas).

The crate knows nothing about the store, pause or settings. It converts,
validates and answers; what to do with the data is the sink's business.

Status: both receivers, the conversion and partial success are implemented and
tested on Linux. Reachability from a Docker container is **unverified** until
the Milestone 1 check on a real Mac (design §4 and §10.4): Linux tests cannot
exercise Docker Desktop's `host.docker.internal`.

## API

```rust
let sink: Arc<dyn Sink> = ...;
let servers = loctel_ingest::start(IngestConfig::default(), sink).await;

servers.grpc;   // ListenerStatus, name "otlp-grpc"
servers.http;   // ListenerStatus, name "otlp-http"
servers.shutdown();

// One receiver on its own (what the engine uses, so it can move one port
// while the other keeps serving):
let (status, stop) = loctel_ingest::start_http(addr, sink, DEFAULT_MAX_MESSAGE_BYTES).await;
stop.shutdown();
```

- `Sink` is `trait Sink: Send + Sync + 'static { fn spans(&self, Vec<Span>);
  fn logs(&self, Vec<LogRecord>); }`. Both are called from async request
  handlers, on the runtime's worker threads, so they must return quickly and
  must not block on I/O. An empty batch is never delivered. A sink cannot
  refuse data: pause is the sink dropping what it is given, and the servers
  keep answering success (design §4).
- `IngestConfig`:

  | Field | Default | Meaning |
  |---|---|---|
  | `grpc_addr` | `127.0.0.1:4317` | where the gRPC receiver listens |
  | `http_addr` | `127.0.0.1:4318` | where the HTTP receiver listens |
  | `max_message_bytes` | 64 MiB | largest request, on both transports (see "Limits") |

  Port 0 picks a free port; the real address is in the listener's state.
- `start` binds both sockets, spawns the servers on the **caller's** tokio
  runtime and returns once both have bound or failed. It must be called inside
  a runtime. Dropping the `IngestServers` stops the servers, as does
  `shutdown(self)`. Either stops accepting, lets requests in flight finish and
  returns without waiting for that.
- `start_grpc(addr, sink, max_message_bytes)` and `start_http(..)` start **one**
  receiver and return its `ListenerStatus` with a `ReceiverShutdown`. The handle
  stops that receiver alone (dropping it does too, `shutdown(self)` is the same
  without waiting), and a port that cannot be bound is a `Failed` status with an
  inert handle, like `start`. `start` is these two over one shared stop signal.
  The engine starts its receivers this way so that `EngineHandle::rebind` can move
  the HTTP port without touching the gRPC one (decision 0035).
- `ListenerStatus { name, requested, state }` with
  `ListenerState::{Listening(SocketAddr), Failed(String)}`. The same types are
  used by the MCP server and the engine, so the status box lists every
  endpoint the same way. `Listening` carries the real address, `Failed` the
  operating system's reason ("Address already in use (os error 98)").
- A listener that cannot bind never stops the other (design §4: a taken port
  keeps the other endpoints running). The failure is only in the status: the
  caller shows it.
- `spans_from_request` and `logs_from_request` are public: the conversion is
  pure, so it can be used and tested without a server.

## Endpoints

| Transport | Where | What |
|---|---|---|
| gRPC | `:4317` | `TraceService/Export`, `LogsService/Export`, `MetricsService/Export` |
| HTTP | `:4318` | `POST /v1/traces`, `POST /v1/logs`, `POST /v1/metrics` |

Both listen on loopback by default (design §4). gRPC accepts `gzip`
(`grpc-encoding`). Any other method or path on HTTP is a 404 or 405.

### HTTP formats

- `Content-Type: application/x-protobuf` or `application/json`, compared
  ignoring case and parameters (`application/json; charset=utf-8` works). Any
  other or missing content type is a 415 with a plain-text hint.
- The reply uses the request's content type, with `partial_success` when
  anything was rejected.
- OTLP/JSON, as the spec says: IDs are hex strings, `int64` and `fixed64`
  values may be strings (numbers are accepted too), enums are integers
  (`"kind": 2`), field names are lowerCamelCase and unknown fields are
  ignored. The decoding comes from `opentelemetry-proto`'s `with-serde`
  support.
- `Content-Encoding: gzip` is inflated, with a `Content-Length` or chunked
  (exporters that compress on the fly, such as Go's, send chunked). Any other
  encoding is a 415.
- Errors carry a `google.rpc.Status` body in the request's format (protobuf, or
  `{"code":3,"message":"..."}`): 400 for a body that does not decode or a
  truncated gzip stream (`code` 3), 413 for a body over the limit (`code` 8).
  A 415 has no usable format to answer in, so it is plain text.
- Success replies are written by hand for JSON: `{}` for a clean success and
  `{"partialSuccess":{"rejectedSpans":"2","errorMessage":"..."}}` otherwise.
  The derived serializer of `opentelemetry-proto` emits `"partialSuccess":null`
  and a bare number for the count, which strict parsers may reject.
- `/v1/metrics` is answered `200` without decoding the body (an empty message,
  or `{}` for JSON). It still needs a known content type.

## Limits

- Every request may be up to `max_message_bytes` (64 MiB). The libraries'
  defaults are 4 MiB (tonic) and 2 MB (axum); real exporter batches exceed them
  (design §4, review focus 4).
- gRPC: `max_decoding_message_size(max)` on each service. tonic bounds
  decompression with the same limit, so a gzip bomb fails instead of filling
  memory.
- HTTP: `DefaultBodyLimit::max(max)`, applied to the body **after** gzip is
  inflated (the decompression layer sits inside it). So the limit is on the
  inflated size, whatever the wire size, and a bomb is a 413.
- Do not add tower-http's `RequestBodyLimitLayer` in front of it. It is
  redundant (it limits the compressed size), and it makes the error
  classification wrong: its error travels through the decompression layer as an
  `io::Error`, so axum cannot recognise it and answers 400 instead of 413 for a
  chunked body over the limit. `http_chunked_request_over_the_limit_is_413`
  guards this (decision 0003).

## Conversion rules

`spans_from_request` and `logs_from_request` take an export request and return
`Converted<T> { items, rejected, error_message }`.

| Input | Result |
|---|---|
| span with a trace ID that is missing, not 16 bytes or all zero | rejected |
| span with a span ID that is missing, not 8 bytes or all zero | rejected |
| span with a parent ID that is non-empty and not 8 bytes | rejected |
| parent ID empty, all zero, or equal to the span's own ID | kept, `parent_span_id: None` (a root) |
| log with an empty, all-zero or malformed trace ID or span ID | kept, that ID is `None` (no link) |
| log body that is a string | the string |
| log body that is not a string | the model's display form: numbers and booleans as text, bytes as hex, arrays and maps as compact JSON of the plain value (`{"user":"alice","n":3}`, not the OTLP/JSON AnyValue shape) |
| log without a body | empty string |
| `SpanKind`, `StatusCode` outside the known values | `Unspecified` and `Unset` |
| `SeverityNumber` | `Severity::from_number` (outside 1 to 24 is `Unspecified`) |
| attribute or array element whose `AnyValue` is empty (nothing set) | dropped |
| resource without `service.name` | kept; the model shows it as `unknown_service` |
| span links, dropped-attribute counts, schema URLs, scope attributes and version | not kept (the model has no place for them) |

- One `Arc<Resource>` is shared by every span or log of a `ResourceSpans` or
  `ResourceLogs`.
- A span is rejected for the first thing wrong with it, in the order trace ID,
  span ID, parent ID.
- Logs are never rejected: a log without a trace is still a log (design §4),
  and a lost log helps nobody. So `rejected` is always 0 for logs.
- Duplicates are not the receiver's business. Exporters retry, and the store
  stores a span or log once however many times it arrives.

## Partial success

When anything was rejected, the response carries `partial_success` with
`rejected_spans` (or `rejected_log_records`) and an `error_message`, for
example `rejected 2 of 3 spans: 1 invalid trace_id (needs 16 bytes, not all
zero), 1 invalid span_id (needs 8 bytes, not all zero)`. The valid items of the
same request are still delivered. A request that is entirely rejected is a
normal `200` with the count and never reaches the sink. Exporters treat the
count as "do not retry these".

## Gotchas

- **Conversion is CPU work on the runtime's workers.** It and the sink call
  run inside the handler, not on a blocking thread. Batches are normally a few
  thousand spans, so this is fine at the design's 50,000 items per second. If a
  very large batch is ever seen to stall other connections, move the conversion
  to `spawn_blocking`.
- **Port 0 and the status.** A caller that asks for port 0 must read the real
  address from `ListenerState::Listening`.
- **Shutdown is graceful.** It stops accepting and lets requests in flight
  finish, but does not wait for idle connections: the HTTP server hangs up on idle
  keep-alive connections when the signal arrives
  (`shutdown_closes_idle_keep_alive_connections`).
- **`AnyValue::StringValueStrindex`** (new in `opentelemetry-proto` 0.33) is an
  index into a string table that exists only in the profiles signal. In traces
  and logs there is nothing to look it up in, so it is treated as empty.
- **`opentelemetry-proto` features** come from the plan: `gen-tonic`, `trace`,
  `logs`, `metrics` (the gRPC metrics service type), `with-serde` (JSON).
  `default-features = false`, since `full` pulls in zpages and
  internal logging.
- **No protoc.** `opentelemetry-proto` ships its generated code, so the crate
  builds without `protobuf-compiler`.

## Tests

`crates/loctel-ingest/tests/convert.rs` covers the conversion rules above.
`tests/servers.rs` runs the real servers on port 0 with a recording sink: gRPC
through the generated tonic clients, HTTP through a small raw-socket client so
that headers, framing and bodies are exactly what the test says. It covers
both transports, protobuf and JSON, gzip (Content-Length and chunked), requests
above the library defaults (10 MiB, with and without gzip), the limit and its
413, malformed bodies, unsupported types and encodings, metrics, partial
success, taken ports in either listener, and shutdown.
