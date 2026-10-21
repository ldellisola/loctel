//! The HTTP side: rmcp's Streamable HTTP service nested at `/mcp` in an axum
//! router, behind the `Origin` check, on a loopback socket.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::middleware;
use loctel_ingest::{ListenerState, ListenerStatus};
use loctel_store::Store;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use tokio::net::TcpListener;
use tokio_util::sync::{CancellationToken, DropGuard};

use crate::origin::require_local_origin;
use crate::tools::{LoctelTools, StatusSource};

/// The name the MCP listener goes by in the status.
const LISTENER_NAME: &str = "mcp";

/// Stops the MCP server when [`shutdown`](McpShutdown::shutdown) is called or
/// the handle is dropped. Connections in flight are closed; clients
/// reconnect.
#[must_use = "dropping the handle stops the server"]
#[derive(Debug)]
pub struct McpShutdown {
    _stop: DropGuard,
}

impl McpShutdown {
    /// Stops accepting connections and ends the open sessions. The server
    /// winds down in the background; this does not wait for it.
    pub fn shutdown(self) {
        drop(self);
    }
}

/// Serves the MCP tools on `addr` until the returned handle is shut down or
/// dropped.
///
/// The server speaks MCP over Streamable HTTP at `http://{addr}/mcp`, serving
/// clients of the 2026-07-28 protocol (statelessly) and older ones (with a
/// session) alike. It is read-only and has no authentication, so it is
/// confined to loopback: an `addr` that is not a loopback address is refused,
/// and any request whose `Origin` names another host gets 403.
///
/// A socket that cannot be bound (the port is taken) is not an error: the
/// returned status is [`ListenerState::Failed`] with the reason, the server
/// does not run, and the handle is inert. The caller shows the status, as for
/// the OTLP receivers. Port 0 picks a free port, and the status carries the
/// real address. Must be called inside a tokio runtime, which also runs the
/// server.
pub async fn serve(
    addr: SocketAddr,
    store: Store,
    status: Arc<dyn StatusSource>,
) -> (ListenerStatus, McpShutdown) {
    let stop = CancellationToken::new();
    let shutdown = McpShutdown {
        _stop: stop.clone().drop_guard(),
    };
    let failed = |reason: String| ListenerStatus {
        name: LISTENER_NAME,
        requested: addr,
        state: ListenerState::Failed(reason),
    };

    if !addr.ip().is_loopback() {
        let reason = format!(
            "the MCP server only listens on loopback, not on {}",
            addr.ip()
        );
        return (failed(reason), shutdown);
    }
    let listener = match TcpListener::bind(addr).await {
        Ok(listener) => listener,
        Err(error) => return (failed(error.to_string()), shutdown),
    };
    let actual = match listener.local_addr() {
        Ok(actual) => actual,
        Err(error) => return (failed(error.to_string()), shutdown),
    };

    // Sessions are for the older protocol; 2026-07-28 clients are stateless.
    // The service ends its sessions and streams when `stop` is cancelled.
    let config = StreamableHttpServerConfig::default().with_cancellation_token(stop.child_token());
    let service = StreamableHttpService::new(
        move || Ok(LoctelTools::new(store.clone(), Arc::clone(&status))),
        Arc::new(LocalSessionManager::default()),
        config,
    );
    let router = Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn(require_local_origin));
    tokio::spawn(async move {
        // An error here is the listener failing after it was bound: there is
        // nobody to tell, and a restart is the remedy.
        let _ = axum::serve(listener, router)
            .with_graceful_shutdown(stop.cancelled_owned())
            .await;
    });

    let listening = ListenerStatus {
        name: LISTENER_NAME,
        requested: addr,
        state: ListenerState::Listening(actual),
    };
    (listening, shutdown)
}
