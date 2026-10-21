//! The loctel MCP server.
//!
//! A read-only [Model Context Protocol](https://modelcontextprotocol.io)
//! server over Streamable HTTP, for AI agents that want to know why a request
//! failed (design section 8). [`serve`] mounts it on a loopback socket; the
//! engine starts it next to the OTLP receivers. Nine tools answer from the
//! store: status, a `since` marker, services, the service map, trace search,
//! recent failures, one trace, one span and log search. They give compact
//! summaries by default, with `limit` and `cursor` to page, and return
//! `structuredContent` with an output schema plus the same JSON as text.
//!
//! The tools are on [`LoctelTools`], which can be called without a
//! connection ([`LoctelTools::call`]). `docs/areas/mcp.md` has the tool
//! table and the sizing rules.

mod dto;
mod origin;
mod server;
mod since;
mod tools;
mod trace_view;

pub use dto::*;
pub use server::{McpShutdown, serve};
pub use since::{SinceError, parse_since};
pub use tools::{
    GetSpanArgs, GetTraceArgs, LoctelTools, RecentFailuresArgs, SearchLogsArgs, SearchTracesArgs,
    SinceArgs, StatusSource,
};
pub use trace_view::Detail;
