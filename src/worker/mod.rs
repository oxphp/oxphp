use std::net::SocketAddr;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use hyper::service::service_fn;
use hyper_util::rt::{TokioIo, TokioTimer};
use tokio::net::TcpListener;

use crate::config::ServerConfig;
use crate::events::EventDispatcher;
use crate::executor::ScriptExecutor;
use crate::metrics::Metrics;
use crate::server;
use crate::server::connection;

/// Shared configuration passed to each worker thread.
pub struct WorkerConfig {
    pub listen_addr: SocketAddr,
    pub server_config: ServerConfig,
    pub executor: Arc<dyn ScriptExecutor>,
    pub metrics: Arc<Metrics>,
    pub dispatcher: Arc<EventDispatcher>,
    pub tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
    pub compression_enabled: bool,
    pub shutdown: Arc<AtomicBool>,
}

/// Create a TCP listener with SO_REUSEPORT so multiple threads can bind the same port.
fn create_reuseport_listener(addr: &SocketAddr) -> std::io::Result<std::net::TcpListener> {
    use socket2::{Domain, Protocol, Socket, Type};

    let domain = if addr.is_ipv4() {
        Domain::IPV4
    } else {
        Domain::IPV6
    };
    let socket = Socket::new(domain, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    socket.set_reuse_port(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&(*addr).into())?;
    socket.listen(1024)?;
    Ok(socket.into())
}

/// Spawn N worker threads, each with its own tokio runtime and SO_REUSEPORT listener.
pub fn spawn_workers(n: usize, config: WorkerConfig) -> Vec<JoinHandle<()>> {
    let config = Arc::new(config);
    (0..n)
        .map(|id| {
            let config = Arc::clone(&config);
            std::thread::Builder::new()
                .name(format!("worker-{id}"))
                .spawn(move || worker_main(id, &config))
                .expect("failed to spawn worker thread")
        })
        .collect()
}

fn worker_main(id: usize, config: &WorkerConfig) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("failed to build tokio runtime");

    let std_listener = create_reuseport_listener(&config.listen_addr)
        .expect("failed to create reuseport listener");

    // Per-thread Server — own RouteConfig + FileCache, shared executor/metrics/dispatcher
    let server = Rc::new(server::Server::new(
        &config.server_config,
        Arc::clone(&config.executor),
        Arc::clone(&config.metrics),
        Arc::clone(&config.dispatcher),
        config.compression_enabled,
    ));

    let shutdown = Arc::clone(&config.shutdown);
    // Rc wrapper — avoids atomic refcount bump per accept (TlsAcceptor is Arc internally)
    let tls_acceptor = config.tls_acceptor.clone().map(Rc::new);

    // Pre-build HTTP/1 builder once per worker — reused for every connection.
    // Wrapped in Rc because serve_connection borrows the builder (Connection<'_>).
    let http = {
        let mut builder = hyper::server::conn::http1::Builder::new();
        builder.timer(TokioTimer::new());
        let header_read_timeout = config.server_config.header_read_timeout;
        if header_read_timeout > Duration::ZERO {
            builder.header_read_timeout(header_read_timeout);
        }
        Rc::new(builder)
    };

    let local = tokio::task::LocalSet::new();
    rt.block_on(local.run_until(async move {
        let listener = TcpListener::from_std(std_listener).expect("failed to convert listener");
        tracing::info!(worker_id = id, "Worker thread started");

        accept_loop(listener, server, shutdown, tls_acceptor, http).await;

        tracing::info!(worker_id = id, "Worker thread stopped");
    }));
}

async fn accept_loop(
    listener: TcpListener,
    server: Rc<server::Server>,
    shutdown: Arc<AtomicBool>,
    tls_acceptor: Option<Rc<tokio_rustls::TlsAcceptor>>,
    http: Rc<hyper::server::conn::http1::Builder>,
) {
    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }

        let (stream, remote_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                tracing::error!(error = %e, "Failed to accept connection");
                continue;
            }
        };

        let server = Rc::clone(&server);
        let tls = tls_acceptor.clone(); // Rc clone — non-atomic
        let http = Rc::clone(&http); // Rc clone — non-atomic
        tokio::task::spawn_local(async move {
            if let Err(e) = handle_connection(stream, remote_addr, server, tls, &http).await {
                tracing::error!(
                    remote_addr = %remote_addr,
                    error = %e,
                    "Connection error"
                );
            }
        });
    }
}

async fn handle_connection(
    stream: tokio::net::TcpStream,
    remote_addr: SocketAddr,
    server: Rc<server::Server>,
    tls_acceptor: Option<Rc<tokio_rustls::TlsAcceptor>>,
    http: &hyper::server::conn::http1::Builder,
) -> Result<(), crate::types::BoxError> {
    server.metrics.connection_opened();
    let _guard = ConnectionGuard(Arc::clone(&server.metrics));

    // Rc::clone per request — non-atomic refcount increment, cheaper than Arc::clone
    let service = service_fn(move |req| {
        let server = Rc::clone(&server);
        async move { connection::handle_request(req, &server, remote_addr).await }
    });

    if let Some(ref acceptor) = tls_acceptor {
        let tls_stream = acceptor.accept(stream).await?;
        let io = TokioIo::new(tls_stream);
        http.serve_connection(io, service)
            .await
            .map_err(|e| format!("connection error: {e}"))?;
    } else {
        let io = TokioIo::new(stream);
        http.serve_connection(io, service)
            .await
            .map_err(|e| format!("connection error: {e}"))?;
    }

    Ok(())
}

/// RAII guard that calls `Metrics::connection_closed()` on drop.
struct ConnectionGuard(Arc<Metrics>);

impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.connection_closed();
    }
}
