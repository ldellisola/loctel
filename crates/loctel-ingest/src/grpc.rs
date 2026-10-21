//! The OTLP gRPC receiver: trace, logs and metrics services.

use std::future::Future;
use std::sync::Arc;

use opentelemetry_proto::tonic::collector::logs::v1::logs_service_server::{
    LogsService, LogsServiceServer,
};
use opentelemetry_proto::tonic::collector::logs::v1::{
    ExportLogsServiceRequest, ExportLogsServiceResponse,
};
use opentelemetry_proto::tonic::collector::metrics::v1::metrics_service_server::{
    MetricsService, MetricsServiceServer,
};
use opentelemetry_proto::tonic::collector::metrics::v1::{
    ExportMetricsServiceRequest, ExportMetricsServiceResponse,
};
use opentelemetry_proto::tonic::collector::trace::v1::trace_service_server::{
    TraceService, TraceServiceServer,
};
use opentelemetry_proto::tonic::collector::trace::v1::{
    ExportTraceServiceRequest, ExportTraceServiceResponse,
};
use tokio::net::TcpListener;
use tonic::codec::CompressionEncoding;
use tonic::transport::Server;
use tonic::transport::server::TcpIncoming;
use tonic::{Request, Response, Status};

use crate::sink::{Sink, export_logs, export_traces};

struct Traces(Arc<dyn Sink>);
struct Logs(Arc<dyn Sink>);
/// Metrics are accepted and dropped, so exporters never log errors.
struct Metrics;

#[tonic::async_trait]
impl TraceService for Traces {
    async fn export(
        &self,
        request: Request<ExportTraceServiceRequest>,
    ) -> Result<Response<ExportTraceServiceResponse>, Status> {
        Ok(Response::new(export_traces(&*self.0, request.into_inner())))
    }
}

#[tonic::async_trait]
impl LogsService for Logs {
    async fn export(
        &self,
        request: Request<ExportLogsServiceRequest>,
    ) -> Result<Response<ExportLogsServiceResponse>, Status> {
        Ok(Response::new(export_logs(&*self.0, request.into_inner())))
    }
}

#[tonic::async_trait]
impl MetricsService for Metrics {
    async fn export(
        &self,
        _request: Request<ExportMetricsServiceRequest>,
    ) -> Result<Response<ExportMetricsServiceResponse>, Status> {
        Ok(Response::new(ExportMetricsServiceResponse::default()))
    }
}

/// Serves the three OTLP services on `listener` until `shutdown` completes.
/// Messages up to `max_message_bytes` are accepted, compressed (gzip) or not,
/// where tonic's own limit is 4 MiB.
pub(crate) async fn serve(
    listener: TcpListener,
    sink: Arc<dyn Sink>,
    max_message_bytes: usize,
    shutdown: impl Future<Output = ()>,
) -> Result<(), tonic::transport::Error> {
    let traces = TraceServiceServer::new(Traces(Arc::clone(&sink)))
        .accept_compressed(CompressionEncoding::Gzip)
        .max_decoding_message_size(max_message_bytes);
    let logs = LogsServiceServer::new(Logs(sink))
        .accept_compressed(CompressionEncoding::Gzip)
        .max_decoding_message_size(max_message_bytes);
    let metrics = MetricsServiceServer::new(Metrics)
        .accept_compressed(CompressionEncoding::Gzip)
        .max_decoding_message_size(max_message_bytes);

    Server::builder()
        .add_service(traces)
        .add_service(logs)
        .add_service(metrics)
        .serve_with_incoming_shutdown(TcpIncoming::from(listener), shutdown)
        .await
}
