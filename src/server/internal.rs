use std::convert::Infallible;
use std::sync::Arc;

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;

use crate::config::Config;
use crate::executor::ScriptExecutor;
use crate::metrics::Metrics;
use crate::types::{full_body, ResponseBody};

/// Run the internal HTTP server for health, metrics, and config endpoints.
/// This server listens on a separate port and is only started when `INTERNAL_ADDR` is set.
pub async fn run_internal_server(
    addr: &str,
    metrics: Arc<Metrics>,
    config: Arc<Config>,
    executor: Arc<dyn ScriptExecutor>,
) -> Result<(), crate::types::BoxError> {
    let listener = TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;
    tracing::info!(addr = %local_addr, "Internal server listening");

    loop {
        let (stream, _remote) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!(error = %e, "Internal server accept error");
                continue;
            }
        };

        let metrics = Arc::clone(&metrics);
        let config = Arc::clone(&config);
        let executor = Arc::clone(&executor);

        tokio::spawn(async move {
            let service = service_fn(move |req| {
                let metrics = Arc::clone(&metrics);
                let config = Arc::clone(&config);
                let executor = Arc::clone(&executor);
                async move { handle_internal_request(req, &metrics, &config, &*executor) }
            });

            let io = TokioIo::new(stream);
            let builder = Builder::new(hyper_util::rt::TokioExecutor::new());
            if let Err(e) = builder.serve_connection(io, service).await {
                tracing::debug!(error = %e, "Internal server connection error");
            }
        });
    }
}

fn handle_internal_request(
    req: Request<Incoming>,
    metrics: &Metrics,
    config: &Config,
    executor: &dyn ScriptExecutor,
) -> Result<Response<ResponseBody>, Infallible> {
    let response = match req.uri().path() {
        "/health" => health_response(metrics, executor),
        "/metrics" => metrics_response(metrics),
        "/config" => config_response(config),
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(full_body(Bytes::from_static(b"404 Not Found")))
            .unwrap(),
    };
    Ok(response)
}

fn health_response(metrics: &Metrics, executor: &dyn ScriptExecutor) -> Response<ResponseBody> {
    let executor_healthy = executor.is_healthy();
    let status_str = if executor_healthy { "ok" } else { "degraded" };
    let http_status = if executor_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let body = serde_json::json!({
        "status": status_str,
        "uptime_secs": metrics.uptime().as_secs(),
        "total_requests": metrics.total_requests(),
        "active_connections": metrics.active_connections(),
        "executor_healthy": executor_healthy,
    });

    Response::builder()
        .status(http_status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(full_body(Bytes::from(body.to_string())))
        .unwrap()
}

fn metrics_response(metrics: &Metrics) -> Response<ResponseBody> {
    let body = metrics.to_prometheus();

    Response::builder()
        .status(StatusCode::OK)
        .header(
            http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )
        .body(full_body(Bytes::from(body)))
        .unwrap()
}

fn config_response(config: &Config) -> Response<ResponseBody> {
    let body = config.to_json();

    Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(full_body(Bytes::from(body.to_string())))
        .unwrap()
}
