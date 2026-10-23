//! The nine tools, and the rmcp handler that serves them.
//!
//! Every tool is read-only and answers with a compact summary by default,
//! with explicit `limit` and `cursor` arguments (design section 8). A tool
//! returns `structuredContent` that matches its output schema, and rmcp adds
//! the same JSON as text for clients that only read text.
//!
//! The tool methods are plain functions of the store, so [`LoctelTools::call`]
//! runs them without a connection, which is how `tests/tools.rs` drives them.
//! To add a tool, see `.claude/skills/add-mcp-tool/SKILL.md`.

use std::sync::Arc;

use loctel_model::{Severity, SpanId, TraceId};
use loctel_query::logql::{self, LogQuery};
use loctel_query::traceql::{self, TraceQuery};
use loctel_store::{LogFilter, PageRequest, Store, TimeWindow, TraceFilter, trace_exception};
use rmcp::handler::server::tool::IntoCallToolResult;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::{CallToolResponse, CallToolResult, ContentBlock, ErrorData, Tool};
use rmcp::{Json, ServerHandler, tool, tool_handler, tool_router};
use schemars::JsonSchema;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::dto::{
    ExceptionDto, FailureDto, GetSpanResult, GetTraceResult, LogDto, LogLevel, MarkerDto,
    RecentFailuresResult, SearchLogsResult, SearchTracesResult, ServiceDto, ServiceListDto,
    ServiceMapDto, SpanBriefDto, SpanDetailDto, StatusDto, TraceSummaryDto, rfc3339, truncate,
};
use crate::since::parse_since;
use crate::trace_view::{self, Detail};

/// What `get_status` reports. The engine implements it, because only it knows
/// whether receiving is paused and how fast data is coming in.
pub trait StatusSource: Send + Sync + 'static {
    fn status(&self) -> StatusDto;
}

/// The text of the error for a trace that is not held.
const TRACE_NOT_FOUND: &str = "trace not found";
/// The text of the error for a span that is not in its trace.
const SPAN_NOT_FOUND: &str = "span not found";

/// The most characters of text a tool's default answer has, on the fixtures of
/// `tests/budget.rs` (the failing checkout, 50 rounds of health checks and a
/// 10,000-span trace): about 6k tokens, under the design's target of about 8k
/// (section 8). The test fails when a default grows past it; lower the default
/// `limit` of the tool that did, never this.
pub const ANSWER_TEXT_BUDGET: usize = 24_000;

// Defaults and caps (design section 8: small answers first).
const TRACES_DEFAULT: usize = 20;
const TRACES_MAX: usize = 100;
const FAILURES_DEFAULT: usize = 5;
const FAILURES_MAX: usize = 20;
const TRACE_SPANS_DEFAULT: usize = 200;
const TRACE_SPANS_MAX: usize = 2_000;
const LOGS_DEFAULT: usize = 50;
const LOGS_MAX: usize = 500;
/// Logs shown with one span.
const SPAN_LOGS_MAX: usize = 50;
/// Error logs shown with one failed trace.
const ERROR_LOGS_MAX: usize = 10;
/// The most pages of error logs read to find a failed trace's first ten.
const ERROR_LOG_SCAN_PAGES: usize = 10;

// --- arguments ------------------------------------------------------------------

/// Arguments of the tools that only take a time window.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SinceArgs {
    /// Only spans that started at or after this time: a marker from
    /// get_marker (a decimal string) or an RFC 3339 time such as
    /// 2026-10-21T08:00:00Z. Default: everything stored.
    pub since: Option<String>,
}

/// Arguments of `search_traces`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchTracesArgs {
    /// Only traces that started at or after this time: a marker from
    /// get_marker (a decimal string) or an RFC 3339 time.
    pub since: Option<String>,
    /// Only traces with a span from this service (exact name).
    pub service: Option<String>,
    /// Only traces that failed or have errors inside. Default false.
    pub errors_only: Option<bool>,
    /// Only traces that lasted at least this many milliseconds.
    pub min_duration_ms: Option<f64>,
    /// A TraceQL query, written as in Grafana Tempo: only traces in which it
    /// matches at least one span, each listing those spans in
    /// `matched_span_ids`. For example, the spans named POST /charges that have
    /// an ancestor from the service web-bff:
    /// `{ resource.service.name = "web-bff" } >> { name = "POST /charges" }`.
    /// Regular expressions (`=~`, `!~`) are anchored, as in Tempo: `{ name =~
    /// "POST" }` does not match `POST /charges`. Syntax loctel does not
    /// support yet (select, by, metrics functions) is an error that names the
    /// column. Combines with the other filters. Blank means no query.
    pub query: Option<String>,
    /// Most traces to return. Default 20, at most 100.
    pub limit: Option<u32>,
    /// The next_cursor of the previous page.
    pub cursor: Option<String>,
}

/// Arguments of `recent_failures`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecentFailuresArgs {
    /// Only traces that started at or after this time: a marker from
    /// get_marker (a decimal string) or an RFC 3339 time.
    pub since: Option<String>,
    /// Most failed traces to return, and most traces with errors inside.
    /// Default 5, at most 20.
    pub limit: Option<u32>,
}

/// Arguments of `get_trace`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetTraceArgs {
    /// The trace ID: 32 hexadecimal characters.
    pub trace_id: String,
    /// `summary` (default): the path to the failure and the top levels first.
    /// `full`: every span, oldest first.
    pub detail: Option<Detail>,
    /// Most spans to return. Default 200, at most 2000.
    pub limit: Option<u32>,
    /// The next_cursor of the previous page, with the same detail.
    pub cursor: Option<String>,
}

/// Arguments of `get_span`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetSpanArgs {
    /// The trace ID: 32 hexadecimal characters.
    pub trace_id: String,
    /// The span ID: 16 hexadecimal characters.
    pub span_id: String,
}

/// Arguments of `search_logs`.
#[derive(Debug, Default, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SearchLogsArgs {
    /// Only logs at or after this time: a marker from get_marker (a decimal
    /// string) or an RFC 3339 time.
    pub since: Option<String>,
    /// Only logs from this service (exact name).
    pub service: Option<String>,
    /// Only logs of this level or worse.
    pub min_level: Option<LogLevel>,
    /// Only logs whose message contains this text, ignoring case.
    pub contains: Option<String>,
    /// Only logs emitted in this trace: 32 hexadecimal characters.
    pub trace_id: Option<String>,
    /// A LogQL query, written as in Grafana Loki: a stream selector and then
    /// filters, such as
    /// `{service_name="payments-api"} |= "declined" | level = "error"`. The
    /// labels are the resource and log attributes with every character outside
    /// `[A-Za-z0-9_]` turned into `_` (`service.name` is `service_name`), plus
    /// `level` (trace, debug, info, warn, error, fatal or unknown),
    /// `scope_name`, `trace_id` and `span_id`. Label matchers are anchored;
    /// line filters (`|~`, `!~`) are not. Syntax loctel does not support yet
    /// (line_format, label_format, metric queries) is an error that names the
    /// column. Combines with the other filters. Blank means no query.
    pub query: Option<String>,
    /// Most logs to return. Default 50, at most 500.
    pub limit: Option<u32>,
    /// The next_cursor of the previous page.
    pub cursor: Option<String>,
}

// --- the handler -------------------------------------------------------------------

/// The MCP tools over a store and a status source. Cloning is cheap (the
/// store is shared), and the HTTP server makes one per session.
#[derive(Clone)]
pub struct LoctelTools {
    store: Store,
    status: Arc<dyn StatusSource>,
}

#[tool_router]
impl LoctelTools {
    #[tool(
        description = "Is loctel receiving? Whether receiving is paused, each endpoint (OTLP gRPC, \
            OTLP HTTP, MCP) with its address and any error, spans and logs received per second \
            over the last 5 seconds, how many traces, spans and logs are held, and the discard \
            rules (what each drops, and whether its query has an error), whether the rules file \
            has an error, the share of what arrived that the rules dropped, and the disk: how \
            many bytes loctel keeps in temporary files, its cap, and the error if it cannot \
            write.",
        annotations(
            title = "loctel status",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn get_status(&self) -> Result<Json<StatusDto>, String> {
        let mut status = self.status.status();
        // The rules are the user's text, and the file error is a parser's:
        // either can be long.
        for rule in &mut status.rules {
            rule.name = truncate(&rule.name);
            rule.error = rule.error.as_deref().map(truncate);
        }
        status.rules_file_error = status.rules_file_error.as_deref().map(truncate);
        status.spill_error = status.spill_error.as_deref().map(truncate);
        Ok(Json(status))
    }

    #[tool(
        description = "The current time on loctel's clock, as a marker. Call it before sending a \
            request to the service under test, then pass the marker as `since` to recent_failures, \
            search_traces, search_logs and the others to see only what happened after it.",
        annotations(
            title = "Get a since marker",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn get_marker(&self) -> Result<Json<MarkerDto>, String> {
        let now = self.store.now_unix_nano();
        Ok(Json(MarkerDto {
            marker: now.to_string(),
            time: rfc3339(now),
        }))
    }

    #[tool(
        description = "The services that sent spans: requests handled (server and consumer \
            spans), how many errored, the error rate and when each was last seen. `since` limits \
            it to spans that started after a marker or an RFC 3339 time.",
        annotations(
            title = "List services",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn list_services(
        &self,
        Parameters(args): Parameters<SinceArgs>,
    ) -> Result<Json<ServiceListDto>, String> {
        let window = window(args.since.as_deref())?;
        let services = self.store.services(window);
        Ok(Json(ServiceListDto {
            services: services.iter().map(ServiceDto::from).collect(),
        }))
    }

    #[tool(
        description = "The service map: services, databases, queues and external hosts as nodes, \
            and the calls between them as edges, each with call and error counts. `since` limits \
            it to spans that started after a marker or an RFC 3339 time.",
        annotations(
            title = "Get the service map",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn get_service_map(
        &self,
        Parameters(args): Parameters<SinceArgs>,
    ) -> Result<Json<ServiceMapDto>, String> {
        let window = window(args.since.as_deref())?;
        Ok(Json(ServiceMapDto::from(&self.store.service_map(window))))
    }

    #[tool(
        description = "Find traces, newest first, as one-row summaries: root span, services, \
            start, duration, span count, status (ok, errors_inside, failed), where it failed, and \
            the first exception. Filter by `since`, `service`, `errors_only` and \
            `min_duration_ms`, or search with a TraceQL `query`, with the syntax of Grafana Tempo, \
            for example { resource.service.name = \"web-bff\" } >> { name = \"POST /charges\" } \
            (the spans named POST /charges that have an ancestor from web-bff). Structural \
            operators `>`, `<`, `>>`, `<<`, `~` and the spanset operators `&&` and `||` work, and \
            so does `| count() > 3`. Regular expressions (`=~`, `!~`) are anchored, as in Tempo. \
            Each trace the query matches lists the spans it matched in `matched_span_ids`. Syntax \
            loctel does not support yet is an error with its column. Pages: `limit` (default 20, \
            at most 100) and `cursor`.",
        annotations(
            title = "Search traces",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn search_traces(
        &self,
        Parameters(args): Parameters<SearchTracesArgs>,
    ) -> Result<Json<SearchTracesResult>, String> {
        let filter = TraceFilter {
            since_unix_nano: since(args.since.as_deref())?,
            service: args.service.filter(|service| !service.is_empty()),
            errors_only: args.errors_only.unwrap_or(false),
            // A float cast saturates, and a negative or missing number is 0.
            min_duration_nanos: args.min_duration_ms.map(|ms| (ms * 1_000_000.0) as u64),
            query: traceql_query(args.query.as_deref())?,
        };
        let page = self.store.search_traces(
            &filter,
            &PageRequest {
                limit: page_limit(args.limit, TRACES_DEFAULT, TRACES_MAX),
                cursor: cursor(args.cursor),
            },
        );
        Ok(Json(SearchTracesResult {
            traces: page.items.iter().map(TraceSummaryDto::from).collect(),
            next_cursor: page.next_cursor,
        }))
    }

    #[tool(
        description = "Why did my last request fail? The most recent failed traces (the root span \
            is erroring), each with the span it failed in, its exception with stack trace, and the \
            trace's error logs; and, listed apart, traces where the request worked but a span \
            inside errored (a call that failed and was retried). Pass a marker from get_marker as \
            `since` to see only failures after it. `limit` defaults to 5, at most 20.",
        annotations(
            title = "Recent failures",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn recent_failures(
        &self,
        Parameters(args): Parameters<RecentFailuresArgs>,
    ) -> Result<Json<RecentFailuresResult>, String> {
        let limit = page_limit(args.limit, FAILURES_DEFAULT, FAILURES_MAX);

        // One pass over the store's index, newest first, that stops when both
        // lists are full, and builds a summary for what it returns and for
        // nothing else (the store's `recent_failures`).
        let found = self
            .store
            .recent_failures(since(args.since.as_deref())?, limit);

        Ok(Json(RecentFailuresResult {
            failed: found
                .failed
                .iter()
                .filter_map(|summary| self.failure(summary))
                .collect(),
            errors_inside: found
                .errors_inside
                .iter()
                .map(TraceSummaryDto::from)
                .collect(),
        }))
    }

    #[tool(
        description = "One trace as a list of spans (parent_span_id and start_offset_ms give the \
            tree), with the trace summary. By default (detail=summary) the first page is the path \
            from the root to the failed span plus every span at depth 2 or less; `omitted` counts \
            the spans after the page and `next_cursor` continues with the rest. detail=full lists \
            every span, oldest first. `limit` defaults to 200, at most 2000. Use get_span for \
            one span in full.",
        annotations(
            title = "Get a trace",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn get_trace(
        &self,
        Parameters(args): Parameters<GetTraceArgs>,
    ) -> Result<Json<GetTraceResult>, String> {
        let id = parse_trace_id(&args.trace_id)?;
        let snapshot = self
            .store
            .trace(id)
            .ok_or_else(|| TRACE_NOT_FOUND.to_owned())?;
        let summary = self
            .store
            .summary(id)
            .ok_or_else(|| TRACE_NOT_FOUND.to_owned())?;
        let page = trace_view::select(
            &snapshot,
            args.detail.unwrap_or_default(),
            page_limit(args.limit, TRACE_SPANS_DEFAULT, TRACE_SPANS_MAX),
            cursor(args.cursor).as_deref(),
        )
        .map_err(|error| error.to_string())?;
        let trace_start = trace_start(&snapshot.spans);
        Ok(Json(GetTraceResult {
            summary: TraceSummaryDto::from(&summary),
            spans: page
                .spans
                .iter()
                .map(|span| SpanBriefDto::new(span, trace_start))
                .collect(),
            omitted: page.omitted as u64,
            next_cursor: page.next_cursor,
        }))
    }

    #[tool(
        description = "Everything about one span: attributes, events, resource, status message, \
            its exception with stack trace, and the logs emitted in it (at most 50, oldest \
            first).",
        annotations(
            title = "Get a span",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn get_span(
        &self,
        Parameters(args): Parameters<GetSpanArgs>,
    ) -> Result<Json<GetSpanResult>, String> {
        let trace = parse_trace_id(&args.trace_id)?;
        let span_id = parse_span_id(&args.span_id)?;
        let snapshot = self
            .store
            .trace(trace)
            .ok_or_else(|| TRACE_NOT_FOUND.to_owned())?;
        let span = snapshot
            .spans
            .iter()
            .find(|span| span.span_id == span_id)
            .ok_or_else(|| SPAN_NOT_FOUND.to_owned())?;
        let logs = self.store.logs_for_span(trace, span_id);
        Ok(Json(GetSpanResult {
            span: SpanDetailDto::new(span, trace_start(&snapshot.spans)),
            logs: logs.iter().take(SPAN_LOGS_MAX).map(LogDto::from).collect(),
        }))
    }

    #[tool(
        description = "Search log lines, newest first, with time, level, service, message and \
            the trace and span they belong to. Filter by `since`, `service`, `min_level`, \
            `contains` (case-insensitive text) and `trace_id`, or search with a LogQL `query`, \
            with the syntax of Grafana Loki, for example \
            {service_name=\"payments-api\"} |= \"declined\" | level = \"error\" (labels are the \
            resource and log attributes with `.` turned into `_`, plus `level`, `scope_name`, \
            `trace_id` and `span_id`; label matchers are anchored, line filters are not). Syntax \
            loctel does not support yet is an error with its column. Pages: `limit` (default 50, \
            at most 500) and `cursor`.",
        annotations(
            title = "Search logs",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    fn search_logs(
        &self,
        Parameters(args): Parameters<SearchLogsArgs>,
    ) -> Result<Json<SearchLogsResult>, String> {
        let filter = LogFilter {
            since_unix_nano: since(args.since.as_deref())?,
            until_unix_nano: None,
            service: args.service.filter(|service| !service.is_empty()),
            min_severity: args.min_level.map(Severity::from),
            levels: None,
            contains: args.contains,
            trace_id: args.trace_id.as_deref().map(parse_trace_id).transpose()?,
            span_id: None,
            query: logql_query(args.query.as_deref())?,
        };
        let page = self.store.search_logs(
            &filter,
            &PageRequest {
                limit: page_limit(args.limit, LOGS_DEFAULT, LOGS_MAX),
                cursor: cursor(args.cursor),
            },
        );
        Ok(Json(SearchLogsResult {
            logs: page.items.iter().map(LogDto::from).collect(),
            next_cursor: page.next_cursor,
        }))
    }
}

#[tool_handler(
    name = "loctel",
    instructions = "loctel receives the OpenTelemetry traces and logs of the services running on \
        this machine. It is read-only. To find out why a request failed, call recent_failures: it \
        names the span the request failed in, the exception and the error logs. To look at one \
        request, call get_marker first, send the request, then pass the marker as `since`. \
        search_traces and search_logs also take a `query`: TraceQL and LogQL, written as in \
        Grafana Tempo and Loki. Results are summaries; drill down with get_trace and get_span. \
        Times are RFC 3339 in UTC."
)]
impl ServerHandler for LoctelTools {}

impl LoctelTools {
    pub fn new(store: Store, status: Arc<dyn StatusSource>) -> LoctelTools {
        LoctelTools { store, status }
    }

    /// The tools as an MCP client lists them: name, description, input and
    /// output schema.
    pub fn definitions() -> Vec<Tool> {
        Self::tool_router().list_all()
    }

    /// Calls a tool by name, as `tools/call` does, without a connection.
    ///
    /// A tool that fails (an unknown trace, bad arguments, an unknown tool)
    /// gives a result with `is_error` set and its reason as the only text, the
    /// way the server answers a client. `args` is the argument object; `null`
    /// means none.
    pub async fn call(&self, name: &str, args: Value) -> CallToolResult {
        match self.dispatch(name, args) {
            Ok(CallToolResponse::Complete(result)) => result,
            Ok(_) => text_error("the tool did not complete".to_owned()),
            Err(error) => text_error(error.message.into_owned()),
        }
    }

    /// Routes by name. Keep it in step with the `#[tool]` methods above:
    /// `call_dispatches_every_registered_tool` fails when it is not.
    fn dispatch(&self, name: &str, args: Value) -> Result<CallToolResponse, ErrorData> {
        let args = match args {
            Value::Null => Value::Object(Map::new()),
            Value::Object(_) => args,
            _ => return Err(invalid_params("arguments must be a JSON object".to_owned())),
        };
        match name {
            "get_status" => self.get_status().into_call_tool_result(),
            "get_marker" => self.get_marker().into_call_tool_result(),
            "list_services" => with_args(args, |a| self.list_services(a)),
            "get_service_map" => with_args(args, |a| self.get_service_map(a)),
            "search_traces" => with_args(args, |a| self.search_traces(a)),
            "recent_failures" => with_args(args, |a| self.recent_failures(a)),
            "get_trace" => with_args(args, |a| self.get_trace(a)),
            "get_span" => with_args(args, |a| self.get_span(a)),
            "search_logs" => with_args(args, |a| self.search_logs(a)),
            _ => Err(ErrorData::invalid_params(
                format!("unknown tool: {name}"),
                None,
            )),
        }
    }

    /// One failed trace of `recent_failures`. `None` if the trace was evicted
    /// since it was listed.
    fn failure(&self, summary: &loctel_store::TraceSummary) -> Option<FailureDto> {
        let snapshot = self.store.trace(summary.trace_id)?;
        let failed_in = summary.failed_in.as_ref()?;
        let span = snapshot
            .spans
            .iter()
            .find(|span| span.span_id == failed_in.span_id)?;
        Some(FailureDto {
            trace: TraceSummaryDto::from(summary),
            failed_in_span: SpanBriefDto::new(span, trace_start(&snapshot.spans)),
            exception: trace_exception(&snapshot).as_ref().map(ExceptionDto::from),
            error_logs: self.error_logs(summary.trace_id),
        })
    }

    /// The first ten error-or-worse logs of a trace, oldest first.
    fn error_logs(&self, trace: TraceId) -> Vec<LogDto> {
        let filter = LogFilter {
            trace_id: Some(trace),
            min_severity: Some(Severity::Error),
            ..LogFilter::default()
        };
        // The store pages newest first, so read it all (a trace has few
        // error logs; a runaway one is cut off) and turn it round.
        let mut logs = Vec::new();
        let mut cursor = None;
        for _ in 0..ERROR_LOG_SCAN_PAGES {
            let page = self.store.search_logs(
                &filter,
                &PageRequest {
                    limit: LOGS_MAX,
                    cursor,
                },
            );
            logs.extend(page.items);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        logs.iter()
            .rev()
            .take(ERROR_LOGS_MAX)
            .map(LogDto::from)
            .collect()
    }
}

// --- helpers --------------------------------------------------------------------------

/// Runs a tool with its arguments read from `args`. Arguments that do not fit
/// are a tool error, worded as rmcp words them when a client calls.
fn with_args<A, R>(
    args: Value,
    tool: impl FnOnce(Parameters<A>) -> R,
) -> Result<CallToolResponse, ErrorData>
where
    A: DeserializeOwned,
    R: IntoCallToolResult,
{
    match serde_json::from_value::<A>(args) {
        Ok(args) => tool(Parameters(args)).into_call_tool_result(),
        Err(error) => Err(invalid_params(error.to_string())),
    }
}

fn invalid_params(reason: String) -> ErrorData {
    ErrorData::invalid_params(format!("failed to deserialize parameters: {reason}"), None)
}

fn text_error(text: String) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(text)])
}

/// `limit` clamped to `1..=max`, or `default` when not given.
fn page_limit(limit: Option<u32>, default: usize, max: usize) -> usize {
    limit.map_or(default, |limit| {
        usize::try_from(limit).unwrap_or(usize::MAX).clamp(1, max)
    })
}

/// An empty cursor is no cursor: some agents send `""` for "not given".
fn cursor(cursor: Option<String>) -> Option<String> {
    cursor.filter(|cursor| !cursor.is_empty())
}

/// `since` as nanoseconds, if given.
fn since(since: Option<&str>) -> Result<Option<u64>, String> {
    since
        .map(|text| parse_since(text).map_err(|error| error.to_string()))
        .transpose()
}

/// A blank `query` is no query: some agents send `""` for "not given", as
/// they do for `cursor` and `service`.
fn is_blank(query: &str) -> bool {
    query.trim().is_empty()
}

/// The TraceQL `query` argument, parsed once for the store to share. The text
/// is parsed as given, so the column an error names counts from the agent's
/// own first character.
fn traceql_query(query: Option<&str>) -> Result<Option<Arc<TraceQuery>>, String> {
    query
        .filter(|text| !is_blank(text))
        .map(|text| {
            traceql::parse(text)
                .map(Arc::new)
                .map_err(|error| format!("invalid TraceQL: {error}"))
        })
        .transpose()
}

/// The LogQL `query` argument, parsed once for the store to share.
fn logql_query(query: Option<&str>) -> Result<Option<Arc<LogQuery>>, String> {
    query
        .filter(|text| !is_blank(text))
        .map(|text| {
            logql::parse(text)
                .map(Arc::new)
                .map_err(|error| format!("invalid LogQL: {error}"))
        })
        .transpose()
}

/// `since` as a time window: everything when not given.
fn window(since_text: Option<&str>) -> Result<TimeWindow, String> {
    Ok(TimeWindow {
        since_unix_nano: since(since_text)?.unwrap_or(0),
    })
}

/// When the trace started: its earliest span (the spans are sorted).
fn trace_start(spans: &[loctel_model::Span]) -> u64 {
    spans.first().map_or(0, |span| span.start_unix_nano)
}

fn parse_trace_id(text: &str) -> Result<TraceId, String> {
    TraceId::from_hex(text).map_err(|error| {
        format!(
            "invalid trace_id {:?}: {error} (a trace ID is 32 hexadecimal characters)",
            shorten(text)
        )
    })
}

fn parse_span_id(text: &str) -> Result<SpanId, String> {
    SpanId::from_hex(text).map_err(|error| {
        format!(
            "invalid span_id {:?}: {error} (a span ID is 16 hexadecimal characters)",
            shorten(text)
        )
    })
}

/// The start of a bad argument, for an error message.
fn shorten(text: &str) -> String {
    text.chars().take(64).collect()
}
