pub mod connection;
pub mod response;
pub mod routing;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpStream;

use crate::config::ServerConfig;
use crate::server::response::static_file::FileCache;
use crate::server::routing::RouteConfig;

/// Core HTTP server managing connections, routing, and shutdown.
pub struct Server {
    route_config: RouteConfig,
    file_cache: Arc<FileCache>,
    active_connections: AtomicUsize,
    shutdown: AtomicBool,
}

impl Server {
    /// Create a new server from configuration.
    pub fn new(config: &ServerConfig) -> Self {
        let route_config = RouteConfig::new(config);
        let file_cache = Arc::new(FileCache::new(200));

        Self {
            route_config,
            file_cache,
            active_connections: AtomicUsize::new(0),
            shutdown: AtomicBool::new(false),
        }
    }

    /// Signal the server to stop accepting new connections.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }

    /// Returns true if shutdown has been initiated.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
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

        self.active_connections.fetch_add(1, Ordering::SeqCst);

        let io = TokioIo::new(stream);
        let route_config = self.route_config.clone();
        let file_cache = Arc::clone(&self.file_cache);

        let service = service_fn(move |req| {
            let route_config = route_config.clone();
            let file_cache = Arc::clone(&file_cache);
            async move { connection::handle_request(req, &route_config, &file_cache, remote_addr).await }
        });

        let builder = Builder::new(hyper_util::rt::TokioExecutor::new());
        let result = builder.serve_connection(io, service).await;

        self.active_connections.fetch_sub(1, Ordering::SeqCst);

        if let Err(e) = result {
            return Err(format!("connection error: {e}").into());
        }
        Ok(())
    }
}
