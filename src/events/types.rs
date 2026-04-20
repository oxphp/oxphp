use std::net::SocketAddr;
use std::time::Duration;

use http::request::Parts;
use http::{Method, Response};

use super::Event;
use crate::types::ResponseBody;

// ── Server lifecycle ──

/// Fired during server boot, before binding the listen socket.
pub struct ServerBooting;

impl Event for ServerBooting {
    fn name(&self) -> &'static str {
        "server.booting"
    }
}

/// Fired after the server is listening and ready to accept connections.
pub struct ServerStarted {
    pub listen_addr: String,
}

impl Event for ServerStarted {
    fn name(&self) -> &'static str {
        "server.started"
    }
}

/// Fired when a graceful shutdown has been initiated.
pub struct ShutdownInitiated;

impl Event for ShutdownInitiated {
    fn name(&self) -> &'static str {
        "server.shutdown_initiated"
    }
}

// ── Config ──

/// Fired during configuration loading, before validation.
pub struct ConfigLoading;

impl Event for ConfigLoading {
    fn name(&self) -> &'static str {
        "config.loading"
    }
}

// ── Connection ──

/// Fired when a new TCP connection is accepted.
pub struct ConnectionAccepted {
    pub remote_addr: SocketAddr,
}

impl Event for ConnectionAccepted {
    fn name(&self) -> &'static str {
        "connection.accepted"
    }
}

/// Fired when a TCP connection is closed.
pub struct ConnectionClosed {
    pub remote_addr: SocketAddr,
}

impl Event for ConnectionClosed {
    fn name(&self) -> &'static str {
        "connection.closed"
    }
}

// ── Request ──

/// Fired when an HTTP request is received, before routing.
/// Handlers can set `early_response` to short-circuit the pipeline (e.g., 429).
/// Access method via `parts.method`, path via `parts.uri.path()`.
pub struct RequestReceived {
    pub parts: Parts,
    pub remote_addr: SocketAddr,
    pub request_id: String,
    /// Set by a handler to short-circuit the pipeline with an early response.
    pub early_response: Option<Response<ResponseBody>>,
    /// Plugin metadata accumulated from plugin handlers.
    pub metadata: Vec<(String, String)>,
    /// Profiling mode selected by a plugin (e.g. `ox_profiler`), drained into
    /// `ScriptRequest.profiling_mode` before worker dispatch. `None` means no
    /// plugin asked for a mode — the executor falls back to `ApmOnly` when
    /// APM is enabled, else `Off`.
    pub profiling_mode: Option<crate::profiling::ProfilingMode>,
    /// Optional run identifier that accompanies the profiling mode decision.
    pub profiling_run_id: Option<String>,
}

impl Event for RequestReceived {
    fn name(&self) -> &'static str {
        "request.received"
    }
}

/// Fired after route resolution, before script execution or file serving.
pub struct RouteResolved {
    pub request_id: String,
    pub path: String,
}

impl Event for RouteResolved {
    fn name(&self) -> &'static str {
        "request.route_resolved"
    }
}

/// Fired after the full request is complete (response sent).
pub struct RequestComplete {
    pub request_id: String,
    pub method: Method,
    pub path: String,
    pub status: u16,
    pub duration: Duration,
    pub remote_addr: SocketAddr,
    /// Request body size in bytes (0 for GET/HEAD).
    pub request_body_size: u64,
    /// Response body size in bytes.
    pub response_size: u64,
    /// Plugin metadata propagated through the event pipeline.
    /// Used for extensible string key-value data (trace context, plugin-specific).
    pub metadata: Vec<(String, String)>,
    /// PHP errors captured during script execution (empty for static files).
    pub php_errors: Vec<crate::types::PhpScriptError>,
    /// Finalized span tree for the request. `None` when APM is disabled or no spans.
    pub profile_tree: Option<std::sync::Arc<crate::profiling::SpanTree>>,
    /// Time spent waiting in the worker queue (microseconds).
    pub queue_wait_us: Option<u64>,
    /// PHP script execution time (microseconds).
    pub php_exec_us: Option<u64>,
}

impl Event for RequestComplete {
    fn name(&self) -> &'static str {
        "request.complete"
    }
}

// ── PHP ──

/// Fired just before a PHP script starts executing.
pub struct ScriptExecutionStarting {
    pub request_id: String,
    pub script_path: String,
}

impl Event for ScriptExecutionStarting {
    fn name(&self) -> &'static str {
        "php.script_execution_starting"
    }
}

/// Fired during PHP request startup (RINIT).
pub struct PhpRequestStartup {
    pub request_id: String,
}

impl Event for PhpRequestStartup {
    fn name(&self) -> &'static str {
        "php.request_startup"
    }
}

/// Fired during PHP request shutdown (RSHUTDOWN).
pub struct PhpRequestShutdown {
    pub request_id: String,
}

impl Event for PhpRequestShutdown {
    fn name(&self) -> &'static str {
        "php.request_shutdown"
    }
}

/// Fired after PHP script execution completes.
pub struct ScriptExecutionComplete {
    pub request_id: String,
    pub execution_time_us: u64,
}

impl Event for ScriptExecutionComplete {
    fn name(&self) -> &'static str {
        "php.script_execution_complete"
    }
}

// ── Response ──

/// Fired while building the HTTP response, before sending.
/// Handlers can modify the response (e.g., add headers, replace error page body).
pub struct ResponseBuilding {
    pub request_id: String,
    pub response: Response<ResponseBody>,
    /// Plugin metadata propagated through the event pipeline.
    pub metadata: Vec<(String, String)>,
}

impl Event for ResponseBuilding {
    fn name(&self) -> &'static str {
        "response.building"
    }
}

// ── Error ──

/// Fired when a request times out.
pub struct RequestTimedOut {
    pub request_id: String,
    pub timeout: Duration,
}

impl Event for RequestTimedOut {
    fn name(&self) -> &'static str {
        "error.request_timed_out"
    }
}

/// Fired when an unhandled error occurs during request processing.
pub struct RequestError {
    pub request_id: String,
    pub error: String,
}

impl Event for RequestError {
    fn name(&self) -> &'static str {
        "error.request_error"
    }
}

// ── Service ──

/// Fired when the `/health` endpoint is checked.
pub struct HealthCheckRequested {
    pub executor_healthy: bool,
}

impl Event for HealthCheckRequested {
    fn name(&self) -> &'static str {
        "service.health_check"
    }
}

/// Fired when metrics are collected (e.g., `/metrics` endpoint).
pub struct MetricsCollected;

impl Event for MetricsCollected {
    fn name(&self) -> &'static str {
        "service.metrics_collected"
    }
}
