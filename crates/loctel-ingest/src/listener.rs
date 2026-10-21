//! Listener status, and the one place a socket is bound.

use std::net::SocketAddr;

use tokio::net::TcpListener;

/// Whether a listener is up. Also used by the MCP server and the engine, so
/// the status box shows every endpoint the same way.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListenerState {
    /// Accepting connections on this address (the real one, which differs
    /// from the requested one when the port was 0).
    Listening(SocketAddr),
    /// Could not bind; the message is the operating system's reason.
    Failed(String),
}

/// One endpoint and how it is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListenerStatus {
    /// A stable name: `otlp-grpc`, `otlp-http` (and `mcp` from the MCP crate).
    pub name: &'static str,
    /// The address that was asked for.
    pub requested: SocketAddr,
    pub state: ListenerState,
}

/// Binds `requested`. On failure the status says why, and the caller carries
/// on without this listener.
pub(crate) async fn bind(
    name: &'static str,
    requested: SocketAddr,
) -> Result<(TcpListener, ListenerStatus), ListenerStatus> {
    let failed = |reason: String| ListenerStatus {
        name,
        requested,
        state: ListenerState::Failed(reason),
    };
    let listener = TcpListener::bind(requested)
        .await
        .map_err(|e| failed(e.to_string()))?;
    let actual = listener.local_addr().map_err(|e| failed(e.to_string()))?;
    Ok((
        listener,
        ListenerStatus {
            name,
            requested,
            state: ListenerState::Listening(actual),
        },
    ))
}
