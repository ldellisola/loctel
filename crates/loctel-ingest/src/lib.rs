//! OTLP receivers for loctel.
//!
//! Spans and log records arrive over gRPC (default port 4317) or HTTP (4318,
//! protobuf or JSON, optionally gzip), are converted to `loctel-model` types
//! and handed to a [`Sink`]. Metrics are accepted and dropped. Invalid items
//! are rejected one by one and reported back to the exporter through
//! `partial_success`; they never fail the whole request.
//!
//! See `docs/areas/ingest.md` for the rules and the reasons.

mod convert;
mod grpc;
mod http;
mod listener;
mod sink;

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use tokio::sync::watch;

pub use convert::{Converted, logs_from_request, spans_from_request};
pub use listener::{ListenerState, ListenerStatus};
pub use sink::Sink;

/// The largest request accepted by default, compressed or not, on both
/// transports: 64 MiB. The libraries' own defaults (4 MiB for tonic, 2 MB for
/// axum) are far too small for real exporter batches.
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 64 * 1024 * 1024;

/// Where to listen and how much to accept.
#[derive(Clone, Debug)]
pub struct IngestConfig {
    pub grpc_addr: SocketAddr,
    pub http_addr: SocketAddr,
    /// The largest request, on the wire and after gzip, on both transports.
    pub max_message_bytes: usize,
}

impl Default for IngestConfig {
    /// The standard OTLP ports on loopback, with a 64 MiB request limit.
    fn default() -> Self {
        let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
        IngestConfig {
            grpc_addr: SocketAddr::new(loopback, 4317),
            http_addr: SocketAddr::new(loopback, 4318),
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
        }
    }
}

/// The running receivers. Dropping this stops them, as does
/// [`IngestServers::shutdown`].
#[derive(Debug)]
pub struct IngestServers {
    /// The gRPC receiver, named `otlp-grpc`.
    pub grpc: ListenerStatus,
    /// The HTTP receiver, named `otlp-http`.
    pub http: ListenerStatus,
    // Dropping the sender, or sending `true`, stops both servers.
    stop: watch::Sender<bool>,
}

impl IngestServers {
    /// Stops accepting and lets requests already in flight finish. The
    /// servers wind down in the background; this does not wait for them.
    pub fn shutdown(self) {
        let _ = self.stop.send(true);
    }
}

/// Binds both listeners and serves OTLP on them until the returned
/// [`IngestServers`] is shut down or dropped.
///
/// A listener that cannot bind (a port taken by something else) is reported
/// in its status as [`ListenerState::Failed`] and never stops the other one.
/// Must be called inside a tokio runtime, which also runs the servers.
pub async fn start(config: IngestConfig, sink: Arc<dyn Sink>) -> IngestServers {
    let (stop, stopped) = watch::channel(false);
    let max = config.max_message_bytes;

    let grpc = match listener::bind("otlp-grpc", config.grpc_addr).await {
        Ok((listener, status)) => {
            tokio::spawn(grpc::serve(
                listener,
                Arc::clone(&sink),
                max,
                wait_for_stop(stopped.clone()),
            ));
            status
        }
        Err(status) => status,
    };

    let http = match listener::bind("otlp-http", config.http_addr).await {
        Ok((listener, status)) => {
            tokio::spawn(http::serve(listener, sink, max, wait_for_stop(stopped)));
            status
        }
        Err(status) => status,
    };

    IngestServers { grpc, http, stop }
}

/// Completes when shutdown is requested, or when the [`IngestServers`] that
/// owns the sender is dropped.
async fn wait_for_stop(mut stopped: watch::Receiver<bool>) {
    let _ = stopped.wait_for(|stop| *stop).await;
}
