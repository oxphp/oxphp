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
    route_config: Arc<RouteConfig>,
    file_cache: Arc<FileCache>,
    executor: Arc<dyn ScriptExecutor>,
    metrics: Arc<Metrics>,
    dispatcher: Arc<EventDispatcher>,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
    request_timeout: Duration,
    header_read_timeout: Duration,
    compression_enabled: bool,
    shutdown: AtomicBool,
}

impl Server {
    /// Create a new server from configuration.
    pub fn new(
        config: &ServerConfig,
        executor: Arc<dyn ScriptExecutor>,
        metrics: Arc<Metrics>,
        dispatcher: Arc<EventDispatcher>,
        tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
        compression_enabled: bool,
    ) -> Self {
        let route_config = RouteConfig::new(config);
        let file_cache = Arc::new(FileCache::new(200));

        Self {
            route_config: Arc::new(route_config),
            file_cache,
            executor,
            metrics,
            dispatcher,
            tls_acceptor,
            request_timeout: config.request_timeout,
            header_read_timeout: config.header_read_timeout,
            compression_enabled,
            shutdown: AtomicBool::new(false),
        }
    }

    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
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

        self.metrics.connection_opened();
        let _guard = ConnectionGuard(Arc::clone(&self.metrics));

        let route_config = Arc::clone(&self.route_config);
        let file_cache = Arc::clone(&self.file_cache);
        let executor = Arc::clone(&self.executor);
        let metrics = Arc::clone(&self.metrics);
        let dispatcher = Arc::clone(&self.dispatcher);
        let request_timeout = self.request_timeout;
        let compression_enabled = self.compression_enabled;

        let service = service_fn(move |req| {
            let route_config = Arc::clone(&route_config);
            let file_cache = Arc::clone(&file_cache);
            let executor = Arc::clone(&executor);
            let metrics = Arc::clone(&metrics);
            let dispatcher = Arc::clone(&dispatcher);
            async move {
                connection::handle_request(
                    req,
                    &route_config,
                    &file_cache,
                    &executor,
                    remote_addr,
                    &metrics,
                    &dispatcher,
                    request_timeout,
                    compression_enabled,
                )
                .await
            }
        });

        let mut builder = Builder::new(hyper_util::rt::TokioExecutor::new());

        // Apply timeouts — header_read_timeout requires a timer to be set
        builder.http1().timer(hyper_util::rt::TokioTimer::new());
        if self.header_read_timeout > Duration::ZERO {
            builder
                .http1()
                .header_read_timeout(self.header_read_timeout);
        }

        if let Some(ref acceptor) = self.tls_acceptor {
            let tls_stream = acceptor.accept(stream).await?;
            let io = TokioIo::new(tls_stream);
            let result = builder.serve_connection(io, service).await;
            if let Err(e) = result {
                return Err(format!("connection error: {e}").into());
            }
        } else {
            let io = TokioIo::new(stream);
            let result = builder.serve_connection(io, service).await;
            if let Err(e) = result {
                return Err(format!("connection error: {e}").into());
            }
        }

        Ok(())
    }
}
