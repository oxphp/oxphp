pub mod compression;
pub mod connection;
pub mod error_pages;
pub mod internal;
pub mod rate_limit;
pub mod response;
pub mod routing;
pub mod tls;

use std::sync::Arc;
use std::time::Duration;

use crate::config::ServerConfig;
use crate::events::EventDispatcher;
use crate::executor::ScriptExecutor;
use crate::metrics::Metrics;
use crate::server::response::static_file::FileCache;
use crate::server::routing::RouteConfig;

/// Core HTTP server state, one instance per worker thread.
///
/// `RouteConfig` and `FileCache` are owned (per-thread, no contention).
/// `Metrics`, `EventDispatcher`, and `ScriptExecutor` are shared via Arc.
pub struct Server {
    pub(crate) route_config: RouteConfig,
    pub(crate) file_cache: FileCache,
    pub(crate) executor: Arc<dyn ScriptExecutor>,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) dispatcher: Arc<EventDispatcher>,
    pub(crate) request_timeout: Duration,
    pub(crate) compression_enabled: bool,
}

impl Server {
    /// Create a new server from configuration.
    pub fn new(
        config: &ServerConfig,
        executor: Arc<dyn ScriptExecutor>,
        metrics: Arc<Metrics>,
        dispatcher: Arc<EventDispatcher>,
        compression_enabled: bool,
    ) -> Self {
        let route_config = RouteConfig::new(config);
        let file_cache = FileCache::new(200);

        Self {
            route_config,
            file_cache,
            executor,
            metrics,
            dispatcher,
            request_timeout: config.request_timeout,
            compression_enabled,
        }
    }

    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.metrics
    }
}
