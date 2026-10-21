# 0003: How ingest limits requests and treats invalid items

Date: 2026-10-21
Status: accepted

## Context

The Milestone 1 plan (Task 6) pins `tower-http` with the features
`decompression-gzip` and `limit`, and says to raise tonic's
`max_decoding_message_size` and axum's `DefaultBodyLimit` to
`max_message_bytes` (64 MiB). The design (§4) wants limits "well above the
library defaults" (4 MiB for tonic, 2 MB for axum) and gzip supported. For
invalid items, the plan's rules cover spans only: reject a span with an invalid
trace or span ID, and treat a parent that is all zero or equal to the span's own
ID as no parent.

Reading the `limit` feature as a `RequestBodyLimitLayer` in front of the
decompression layer, a chunked request (no `Content-Length`) over the limit
came back as `400`, not `413`: the limit error travels through the
decompression layer wrapped in an `io::Error`, and axum's `Bytes` extractor only
recognises an unwrapped length-limit error. The same request without the extra
layer is a `413`.

## Decision

- **One body limit, on the inflated size.** The HTTP receiver uses
  `DefaultBodyLimit::max(max_message_bytes)` with the decompression layer inside
  it, and no `RequestBodyLimitLayer`. The limit then bounds what a gzip body
  inflates to, which also covers a gzip bomb, and over-limit bodies are `413`
  with or without `Content-Length`. The `limit` feature is dropped from the
  workspace `tower-http` pin because nothing uses it. gRPC uses
  `max_decoding_message_size`, which tonic also applies to decompression.
- **Logs are never rejected.** A log with an empty, all-zero or malformed trace
  ID or span ID is kept, with that ID as `None`. Logs without trace context are
  already first-class (§4), and a rejected log is a lost one.
- **A malformed parent ID rejects the span.** Empty, all-zero and self-parent
  mean "root" (the plan). A parent that is non-empty and not 8 bytes is neither
  a root nor a link, so the span is rejected and counted in `partial_success`
  where the exporter's owner can see it. Turning it into a root would invent a
  second root for the trace.
- **JSON replies are written by hand.** The derived serializer for the OTLP
  response types emits `"partialSuccess":null` and a bare number for the
  rejected count. OTLP/JSON wants `{}` and a string for `int64`.

## Consequences

- A later agent that finds `limit` missing from the plan's pin, or is tempted
  to add a wire-size limit for "defence in depth", should read
  `http_chunked_request_over_the_limit_is_413` first: it fails if the
  classification regresses.
- A request over the limit is read up to the limit before it is refused, so a
  client can make the server buffer up to 64 MiB per request. That is accepted
  for a loopback developer tool.
- Reaching the limit is `OUT_OF_RANGE` over gRPC (tonic's choice, and
  `RESOURCE_EXHAUSTED` for a gzip bomb) and `413` with a `google.rpc.Status` body
  over HTTP.
