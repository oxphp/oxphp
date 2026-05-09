pub mod compression;
pub mod connection;
pub mod error_pages;
pub mod internal;
pub mod rate_limit;
pub mod response;
pub mod routing;
pub mod tls;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpStream;

use std::path::PathBuf;

use crate::config::ServerConfig;
use crate::events::EventDispatcher;
use crate::executor::ScriptExecutor;
use crate::metrics::Metrics;
use crate::server::response::static_file::FileCache;
use crate::server::routing::RouteConfig;

/// RAII guard that calls `Metrics::connection_closed()` on drop.
struct ConnectionGuard(Arc<Metrics>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.connection_closed();
    }
}

/// Core HTTP server managing connections, routing, and shutdown.
pub struct Server {
    pub(crate) route_config: Arc<RouteConfig>,
    pub(crate) file_cache: Arc<FileCache>,
    pub(crate) executor: Arc<dyn ScriptExecutor>,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) dispatcher: Arc<EventDispatcher>,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
    pub(crate) compression_level: i32,
    pub(crate) max_query_body: usize,
    /// Pre-computed `Cache-Control` header value, e.g. `"public, max-age=2592000"`.
    /// `None` = caching disabled.
    pub(crate) static_cache_control: Option<String>,
    /// Pre-configured HTTP builder reused across all connections.
    http_builder: Builder<hyper_util::rt::TokioExecutor>,
    shutdown: Arc<AtomicBool>,
}

impl Server {
    /// Create a new server from configuration.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: &ServerConfig,
        executor: Arc<dyn ScriptExecutor>,
        metrics: Arc<Metrics>,
        dispatcher: Arc<EventDispatcher>,
        tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
        compression_level: i32,
        max_query_body: usize,
        entry_file: Option<PathBuf>,
        worker_mode_enabled: bool,
        static_cache_control: Option<String>,
        static_revalidate: bool,
        shutdown: Arc<AtomicBool>,
    ) -> Self {
        let mut route_config = RouteConfig::new(config, entry_file.as_deref(), worker_mode_enabled);
        if worker_mode_enabled {
            if let Some(entry) = entry_file {
                route_config.set_worker_route(entry);
            }
        }
        let file_cache = Arc::new(FileCache::with_revalidation(200, static_revalidate));

        // Pre-build the HTTP connection builder once — reused for every connection
        let mut http_builder = Builder::new(hyper_util::rt::TokioExecutor::new());
        http_builder
            .http1()
            .timer(hyper_util::rt::TokioTimer::new())
            // Flush the write buffer after every response — reduces latency for
            // pipelined and keep-alive requests that would otherwise wait in buffer.
            .pipeline_flush(true)
            // Use vectored I/O (writev) to send headers + body in a single syscall.
            .writev(true);
        if config.header_read_timeout > Duration::ZERO {
            http_builder
                .http1()
                .header_read_timeout(config.header_read_timeout);
        }
        // HTTP/2: increase flow-control windows from default 64KB to avoid stalls
        // on typical PHP responses (10-500KB). Connection window bounds total
        // concurrent transfer; per-stream window bounds individual responses.
        http_builder
            .http2()
            .initial_connection_window_size(8 * 1024 * 1024)
            .initial_stream_window_size(4 * 1024 * 1024);

        Self {
            route_config: Arc::new(route_config),
            file_cache,
            executor,
            metrics,
            dispatcher,
            tls_acceptor,
            compression_level,
            max_query_body,
            static_cache_control,
            http_builder,
            shutdown,
        }
    }

    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }

    /// Whether TLS is enabled on this server instance.
    pub(crate) fn is_tls(&self) -> bool {
        self.tls_acceptor.is_some()
    }

    /// Signal the server to stop accepting new connections.
    pub fn shutdown(&self) {
        self.executor.shutdown();
        self.shutdown.store(true, Ordering::SeqCst);
    }

    /// Returns true if shutdown has been initiated.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    /// Returns the number of currently active connections.
    pub fn active_connections(&self) -> usize {
        self.metrics.active_connections()
    }

    /// Handle a single TCP connection (may serve multiple HTTP requests via keep-alive).
    pub async fn handle_connection(
        self: &Arc<Self>,
        stream: TcpStream,
        remote_addr: SocketAddr,
    ) -> Result<(), crate::types::BoxError> {
        if self.is_shutdown() {
            return Ok(());
        }

        // Disable Nagle's algorithm — send small responses immediately instead of
        // waiting up to 40ms for more data. Critical for HTTP keep-alive latency.
        let _ = stream.set_nodelay(true);

        self.metrics.connection_opened();
        let _guard = ConnectionGuard(Arc::clone(&self.metrics));

        let server = Arc::clone(self); // 1 Arc clone for the connection
        let service = service_fn(move |req| {
            let server = Arc::clone(&server); // 1 Arc clone per request (was 10)
            async move { connection::handle_request(req, &server, remote_addr).await }
        });

        if let Some(ref acceptor) = self.tls_acceptor {
            let tls_stream = acceptor.accept(stream).await?;
            let io = TokioIo::new(tls_stream);
            let result = self.http_builder.serve_connection(io, service).await;
            if let Err(e) = result {
                return Err(format!("connection error: {e}").into());
            }
        } else {
            let io = TokioIo::new(stream);
            let result = self.http_builder.serve_connection(io, service).await;
            if let Err(e) = result {
                return Err(format!("connection error: {e}").into());
            }
        }

        Ok(())
    }
}
