mod connection;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};

use crate::events::EventDispatcher;
use crate::metrics::Metrics;
use crate::server::response::static_file::FileCache;
use crate::server::routing::RouteConfig;

/// Whether this server instance uses stub or SAPI executor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutorMode {
    Stub,
    Sapi,
}

/// Shared read-only state passed to all worker threads.
///
/// All fields are `Send + Sync`. Workers receive `Arc<SharedState>`.
/// No executor field — stub runs inline, SAPI calls `execute_request()` directly.
pub struct SharedState {
    pub route_config: Arc<RouteConfig>,
    pub file_cache: Arc<FileCache>,
    pub metrics: Arc<Metrics>,
    pub dispatcher: Arc<EventDispatcher>,
    pub request_timeout: Duration,
    pub header_read_timeout: Duration,
    pub compression_enabled: bool,
    pub mode: ExecutorMode,
    shutdown: AtomicBool,
}

impl SharedState {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        route_config: Arc<RouteConfig>,
        file_cache: Arc<FileCache>,
        metrics: Arc<Metrics>,
        dispatcher: Arc<EventDispatcher>,
        request_timeout: Duration,
        header_read_timeout: Duration,
        compression_enabled: bool,
        mode: ExecutorMode,
    ) -> Self {
        Self {
            route_config,
            file_cache,
            metrics,
            dispatcher,
            request_timeout,
            header_read_timeout,
            compression_enabled,
            mode,
            shutdown: AtomicBool::new(false),
        }
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }
}

/// Spawn N worker threads, each with its own TCP listener (SO_REUSEPORT),
/// single-threaded Tokio runtime, and hyper HTTP/1 server.
pub fn spawn_workers(
    state: Arc<SharedState>,
    addr: SocketAddr,
    num_workers: usize,
) -> Vec<JoinHandle<()>> {
    let mut handles = Vec::with_capacity(num_workers);

    for worker_id in 0..num_workers {
        let state = Arc::clone(&state);

        let handle = std::thread::Builder::new()
            .name(format!("worker-{worker_id}"))
            .spawn(move || {
                worker_thread(worker_id, state, addr);
            })
            .expect("failed to spawn worker thread");

        handles.push(handle);
    }

    handles
}

fn worker_thread(worker_id: usize, state: Arc<SharedState>, addr: SocketAddr) {
    // Initialize PHP ZTS thread-local storage for this worker thread
    #[cfg(feature = "php")]
    if state.mode == ExecutorMode::Sapi {
        crate::executor::sapi::php_thread_init();
    }

    // Create socket with SO_REUSEPORT
    let socket = socket2::Socket::new(
        match addr {
            SocketAddr::V4(_) => socket2::Domain::IPV4,
            SocketAddr::V6(_) => socket2::Domain::IPV6,
        },
        socket2::Type::STREAM,
        Some(socket2::Protocol::TCP),
    )
    .expect("failed to create socket");

    socket.set_reuse_address(true).expect("SO_REUSEADDR");
    socket.set_reuse_port(true).expect("SO_REUSEPORT");
    socket.set_nonblocking(true).expect("set_nonblocking");
    socket
        .bind(&addr.into())
        .unwrap_or_else(|e| panic!("bind to {addr}: {e}"));
    socket.listen(1024).expect("listen");

    let std_listener: std::net::TcpListener = socket.into();

    // Pre-build HTTP/1 connection builder once per worker — reused for every connection.
    // Using hyper's http1::Builder directly (not auto::Builder) skips per-connection
    // protocol detection.
    let mut http_builder = http1::Builder::new();
    http_builder.timer(TokioTimer::new());
    if state.header_read_timeout > Duration::ZERO {
        http_builder.header_read_timeout(state.header_read_timeout);
    }

    // Build single-threaded Tokio runtime for this worker
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    runtime.block_on(async move {
        let listener = tokio::net::TcpListener::from_std(std_listener)
            .expect("failed to convert to tokio TcpListener");

        tracing::info!(worker_id, addr = %addr, "Worker thread started");

        match state.mode {
            // SAPI: handle connections inline (no tokio::spawn).
            // PHP blocks the runtime, so concurrent tasks don't help.
            // Zero extra Arc clones — service_fn borrows state directly.
            ExecutorMode::Sapi => {
                accept_loop_inline(&listener, &state, &http_builder).await;
            }
            // Stub: spawn each connection as a Tokio task.
            // Non-blocking executor benefits from concurrent keep-alive connections.
            ExecutorMode::Stub => {
                accept_loop_spawn(&listener, state, &http_builder).await;
            }
        }

        tracing::info!(worker_id, "Worker thread stopped");
    });
}

/// Inline accept loop (SAPI): no tokio::spawn, no extra Arc clones.
async fn accept_loop_inline(
    listener: &tokio::net::TcpListener,
    state: &SharedState,
    http_builder: &http1::Builder,
) {
    loop {
        let accept_result = tokio::select! {
            result = listener.accept() => result,
            _ = shutdown_check(state) => break,
        };

        let (stream, remote_addr) = match accept_result {
            Ok(conn) => conn,
            Err(e) => {
                if state.is_shutdown() {
                    break;
                }
                tracing::error!(error = %e, "Accept error");
                continue;
            }
        };

        state.metrics.connection_opened();

        let service = service_fn(|req| connection::handle_request(req, state, remote_addr));

        let io = TokioIo::new(stream);
        let result = http_builder.serve_connection(io, service).await;
        if let Err(e) = result {
            if !state.is_shutdown() {
                tracing::debug!(remote_addr = %remote_addr, error = %e, "Connection error");
            }
        }

        state.metrics.connection_closed();
    }
}

/// Spawning accept loop (stub): tokio::spawn per connection for concurrency.
async fn accept_loop_spawn(
    listener: &tokio::net::TcpListener,
    state: Arc<SharedState>,
    http_builder: &http1::Builder,
) {
    loop {
        let accept_result = tokio::select! {
            result = listener.accept() => result,
            _ = shutdown_check(&state) => break,
        };

        let (stream, remote_addr) = match accept_result {
            Ok(conn) => conn,
            Err(e) => {
                if state.is_shutdown() {
                    break;
                }
                tracing::error!(error = %e, "Accept error");
                continue;
            }
        };

        state.metrics.connection_opened();

        // 1 Arc clone per connection (for the spawned task).
        // Clone metrics separately so connection_closed() is called after conn finishes.
        let metrics = Arc::clone(&state.metrics);
        let state = Arc::clone(&state);
        let conn = http_builder.serve_connection(
            TokioIo::new(stream),
            service_fn(move |req| {
                // service_fn is FnMut — called for each request on this keep-alive connection.
                // We need to clone state per request because the async block moves it.
                let state = Arc::clone(&state);
                async move { connection::handle_request(req, &state, remote_addr).await }
            }),
        );

        tokio::spawn(async move {
            if let Err(e) = conn.await {
                tracing::debug!(remote_addr = %remote_addr, error = %e, "Connection error");
            }
            metrics.connection_closed();
        });
    }
}

/// Poll shutdown flag periodically. Used in `tokio::select!` to break the accept loop.
async fn shutdown_check(state: &SharedState) {
    loop {
        tokio::time::sleep(Duration::from_millis(250)).await;
        if state.is_shutdown() {
            return;
        }
    }
}

/// Parse WORKER_THREADS env var. Defaults to the number of available CPUs.
pub fn parse_worker_threads() -> usize {
    std::env::var("WORKER_THREADS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n > 0)
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(4)
        })
}
