//! What the inspector panel says about the selected span: the strings of its
//! header, its status, its exception and its two key/value grids, ready to
//! lay out. The view cuts long values (a 20 KB attribute is whole here), draws
//! the stack trace in monospace and the copy buttons; nothing in it formats a
//! value by itself.
//!
//! The strings the inspector's buttons put on the clipboard are here too: the
//! exception ([`exception_text`]) and the whole span ([`span_json`]).

use loctel_model::{AttrValue, ExceptionInfo, SpanId, SpanKind, StatusCode};
use loctel_store::TraceSnapshot;

use super::format;
use super::waterfall::extent;

/// The inspector's text for one span.
#[derive(Clone, Debug, PartialEq)]
pub struct SpanDetails {
    pub name: String,
    pub service: String,
    /// The span kind as a word: `Server`, `Client`, `Internal`, `Producer`,
    /// `Consumer` or `Unspecified`.
    pub kind: String,
    /// The span's length as [`format::duration`] writes it.
    pub duration: String,
    /// Where it starts in the trace: `starts at +200 ms`. The root (anything
    /// that starts with the trace) reads `starts at +0 ms`.
    pub offset: String,
    /// The status code (`Unset`, `OK` or `Error`), followed by ` · ` and the
    /// status message when the span has one: `Error · HTTP 500`.
    pub status: String,
    /// The span's first `exception` event, if it has one.
    pub exception: Option<ExceptionInfo>,
    /// The span's attributes, sorted by key (a stable sort: two with one key
    /// keep their order). Values are written as
    /// [`AttrValue::to_display_string`] writes them, whole.
    pub attributes: Vec<(String, String)>,
    /// The resource's attributes, the same way.
    pub resource: Vec<(String, String)>,
}

/// How many characters of an attribute value the inspector shows before it
/// cuts the rest off behind `… (show all)`.
pub const VALUE_PREVIEW_CHARS: usize = 200;

/// The first [`VALUE_PREVIEW_CHARS`] characters of `value` when it is longer
/// than that (the part the inspector shows, followed by a link that shows the
/// rest), `None` when the whole value fits. It counts characters, never bytes,
/// so it cannot cut inside one.
pub fn value_preview(value: &str) -> Option<&str> {
    let (end, _) = value.char_indices().nth(VALUE_PREVIEW_CHARS)?;
    Some(&value[..end])
}

/// What the exception card's `Copy` puts on the clipboard: the type and the
/// message on the first line (`Type: message`, or whichever of the two there
/// is), then the stack trace as it was sent.
pub fn exception_text(exception: &ExceptionInfo) -> String {
    let mut text = match (exception.type_name.is_empty(), exception.message.is_empty()) {
        (false, false) => format!("{}: {}", exception.type_name, exception.message),
        (false, true) => exception.type_name.clone(),
        (true, _) => exception.message.clone(),
    };
    if let Some(stacktrace) = exception
        .stacktrace
        .as_deref()
        .filter(|trace| !trace.is_empty())
    {
        text.push('\n');
        text.push_str(stacktrace);
    }
    text
}

/// The span `id` as JSON, pretty-printed, for `Copy span as JSON`: every field
/// of the model's `Span` (IDs in hex, times in nanoseconds, attributes as an
/// object, the events, the resource). `None` when the trace has no such span.
pub fn span_json(snapshot: &TraceSnapshot, id: SpanId) -> Option<String> {
    let span = snapshot.spans.iter().find(|span| span.span_id == id)?;
    serde_json::to_string_pretty(span).ok()
}

/// The details of the span `id`, or `None` when the trace has no such span
/// (the placeholder row of the waterfall has none either).
pub fn span_details(snapshot: &TraceSnapshot, id: SpanId) -> Option<SpanDetails> {
    let span = snapshot.spans.iter().find(|span| span.span_id == id)?;
    let (trace_start, _) = extent(&snapshot.spans);
    let offset = span.start_unix_nano.saturating_sub(trace_start);
    // A zero has no natural unit; the trace is mostly read in milliseconds.
    let at = if offset == 0 {
        "0 ms".to_owned()
    } else {
        format::duration(offset)
    };
    Some(SpanDetails {
        name: span.name.clone(),
        service: span.service().to_owned(),
        kind: kind_name(span.kind).to_owned(),
        duration: format::duration(span.duration_nanos()),
        offset: format!("starts at +{at}"),
        status: status_text(span.status, &span.status_message),
        exception: span.exception(),
        attributes: sorted(&span.attributes),
        resource: sorted(&span.resource.attributes),
    })
}

fn kind_name(kind: SpanKind) -> &'static str {
    match kind {
        SpanKind::Unspecified => "Unspecified",
        SpanKind::Internal => "Internal",
        SpanKind::Server => "Server",
        SpanKind::Client => "Client",
        SpanKind::Producer => "Producer",
        SpanKind::Consumer => "Consumer",
    }
}

fn status_text(status: StatusCode, message: &str) -> String {
    let code = match status {
        StatusCode::Unset => "Unset",
        StatusCode::Ok => "OK",
        StatusCode::Error => "Error",
    };
    if message.is_empty() {
        code.to_owned()
    } else {
        format!("{code} \u{b7} {message}")
    }
}

fn sorted(attributes: &[(String, AttrValue)]) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = attributes
        .iter()
        .map(|(key, value)| (key.clone(), value.to_display_string()))
        .collect();
    pairs.sort_by(|a, b| a.0.cmp(&b.0));
    pairs
}
