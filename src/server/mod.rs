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

use crate::config::{H2Config, ServerConfig};
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
    /// Broadcast latch that flips to `true` when draining begins. Each
    /// connection subscribes and, on flip, calls hyper's `graceful_shutdown()`.
    /// Flipped via `send_replace` (stores even with zero receivers) and
    /// observed via `wait_for` (checks the current value before waiting), so
    /// a SIGTERM on an idle server and the accept→subscribe race are both
    /// covered without retaining an extra receiver.
    drain_tx: tokio::sync::watch::Sender<bool>,
}

impl Server {
    /// Create a new server from configuration.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        config: &ServerConfig,
        h2: &H2Config,
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
        //
        // keep_alive PING/PONG detects dead connections (not Window Stall — a
        // client that responds to PINGs but holds flow window at 0 still pins
        // memory; that requires L4 enforcement or a CDN in front).
        // max_concurrent_streams caps per-connection parallelism, bounding the
        // number of stalled streams an attacker can hold open simultaneously.
        // max_header_list_size limits total decoded header bytes (HPACK bomb).
        // max_pending_accept_reset_streams: explicit Rapid Reset (CVE-2023-44487) cap
        {
            let mut h2b = http_builder.http2();
            h2b.timer(hyper_util::rt::TokioTimer::new())
                .initial_connection_window_size(8 * 1024 * 1024)
                .initial_stream_window_size(4 * 1024 * 1024)
                .max_concurrent_streams(h2.max_concurrent_streams)
                .max_pending_accept_reset_streams(h2.max_pending_accept_reset)
                .max_header_list_size(h2.max_header_list_bytes);
            if let Some(interval) = h2.keepalive_interval {
                h2b.keep_alive_interval(interval)
                    .keep_alive_timeout(h2.keepalive_timeout);
            }
        }

        let (drain_tx, _) = tokio::sync::watch::channel(false);

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
            drain_tx,
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
        // Wake every live connection so it can send GOAWAY / Connection: close.
        // send_replace, not send: it stores the value even when no receiver is
        // currently subscribed (SIGTERM on an idle server).
        self.drain_tx.send_replace(true);
        // Latch the bridge drain flag so the worker-thread stream-flush path
        // and the C fiber scheduler observe the shutdown. On host builds
        // (no `php` feature) `bridge::ffi` is the mock and this is a no-op.
        unsafe { crate::bridge::ffi::oxphp_bridge_set_draining() };
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

        // Per-connection close signal. Flips to `true` after `serve_connection`
        // returns (clean close, RST, or error). Each in-flight request races
        // its dispatch.await against this watch so HTTP/2 stream resets and
        // HTTP/1.1 between-request closes proactively cancel the worker.
        let (closed_tx, closed_rx) = tokio::sync::watch::channel(false);

        let server = Arc::clone(self); // 1 Arc clone for the connection
        let service = service_fn({
            let closed_rx = closed_rx.clone();
            move |req| {
                let server = Arc::clone(&server); // 1 Arc clone per request (was 10)
                let closed_rx = closed_rx.clone();
                async move { connection::handle_request(req, &server, remote_addr, closed_rx).await }
            }
        });

        // `wait_for` checks the current value before waiting, so a flip that
        // landed between accept and this subscribe is still observed — no
        // accept→subscribe race.
        let mut drain_rx = self.drain_tx.subscribe();
        let result = if let Some(ref acceptor) = self.tls_acceptor {
            // The handshake itself must also observe the drain latch: rustls
            // has no handshake timeout, and a client that connects and then
            // stalls mid-handshake would otherwise pin active_connections()
            // for the whole drain window.
            // The extra async block discards `wait_for`'s `Ref` return value —
            // it holds a read guard that is not `Send`, and `select!` would
            // otherwise store it in its output enum, making the whole
            // connection future non-spawnable.
            let tls_stream = tokio::select! {
                accepted = acceptor.accept(stream) => accepted?,
                _ = async { drain_rx.wait_for(|&draining| draining).await.ok(); } => {
                    // Deliberate wind-down, not a failure: an Err here would
                    // surface as a Connection-error log line on every deploy.
                    tracing::debug!("TLS handshake abandoned: server draining");
                    return Ok(());
                }
            };
            let io = TokioIo::new(tls_stream);
            self.serve_with_drain(io, service, &mut drain_rx).await
        } else {
            let io = TokioIo::new(stream);
            self.serve_with_drain(io, service, &mut drain_rx).await
        };

        // Notify any in-flight handler that the connection is done. This lets
        // them race their dispatch against the watch and trigger client-abort
        // cancellation on the worker side.
        let _ = closed_tx.send(true);

        if let Err(e) = result {
            return Err(format!("connection error: {e}").into());
        }

        Ok(())
    }

    /// Serve one connection, winding it down gracefully when the drain latch
    /// flips. Shared by the TLS and cleartext arms of `handle_connection` so the
    /// drain/`graceful_shutdown` logic lives in exactly one place.
    async fn serve_with_drain<I, S, B>(
        &self,
        io: I,
        service: S,
        drain_rx: &mut tokio::sync::watch::Receiver<bool>,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>>
    where
        I: hyper::rt::Read + hyper::rt::Write + Unpin + Send + 'static,
        S: hyper::service::Service<
                hyper::Request<hyper::body::Incoming>,
                Response = hyper::Response<B>,
            > + Send
            + 'static,
        S::Future: Send + 'static,
        S::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
        B: hyper::body::Body + Send + 'static,
        B::Data: Send,
        B::Error: Into<Box<dyn std::error::Error + Send + Sync>>,
    {
        let conn = self.http_builder.serve_connection(io, service);
        tokio::pin!(conn);
        tokio::select! {
            res = conn.as_mut() => res,
            // async block: drop wait_for's non-Send `Ref` before yielding.
            _ = async { drain_rx.wait_for(|&draining| draining).await.ok(); } => {
                // GOAWAY (h2) / Connection: close (h1), then finish in-flight work.
                conn.as_mut().graceful_shutdown();
                conn.as_mut().await
            }
        }
    }
}
