#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod logging;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use oxphp::config;
use oxphp::events::EventDispatcher;
use oxphp::executor;
use oxphp::handlers;
use oxphp::metrics::Metrics;
use oxphp::plugin::PluginManager;
use oxphp::server;
use oxphp::types;
use oxphp::worker::{self, WorkerConfig};

fn main() -> Result<(), types::BoxError> {
    let config = Arc::new(config::Config::from_env()?);
    let _log_guard = logging::init(&config.log_level)?;

    tracing::info!(
        listen_addr = %config.server.listen_addr,
        document_root = %config.server.document_root.display(),
        executor = %config.executor_type,
        "OxPHP HTTP server starting"
    );

    // Create metrics early — needed by executor for worker metrics
    let metrics = Arc::new(Metrics::new());

    // Initialize plugins BEFORE PHP startup so MINIT can register plugin
    // functions with Zend (OPcache needs them at compile time).
    let mut dispatcher = EventDispatcher::new();
    let mut plugin_manager = PluginManager::new();
    #[cfg(feature = "plugin-example")]
    plugin_manager.add(Box::new(oxphp::plugins::example::ExamplePlugin::new()));
    plugin_manager.init_all(&mut dispatcher)?;

    #[cfg(feature = "php")]
    {
        let native_fns = plugin_manager.take_native_php_functions();
        if !native_fns.is_empty() {
            oxphp::php::sapi::register_native_plugin_functions(native_fns);
        }
    }

    // Create executor AFTER plugin functions are on the bridge —
    // php_module_startup() (MINIT) registers them with Zend.
    let executor: Arc<dyn executor::ScriptExecutor> =
        Arc::from(executor::create_executor(Arc::clone(&metrics)));

    // Initialize optional rate limiter
    let rate_limiter = if config.rate_limit > 0 {
        let limiter = Arc::new(server::rate_limit::RateLimiter::new(
            config.rate_limit,
            config.rate_window,
        ));
        tracing::info!(
            rate_limit = config.rate_limit,
            rate_window = config.rate_window,
            "Rate limiting enabled"
        );
        Some(limiter)
    } else {
        None
    };

    // Initialize optional TLS
    let tls_acceptor = match (&config.tls_cert, &config.tls_key) {
        (Some(cert), Some(key)) => {
            let acceptor = server::tls::load_tls_config(Path::new(cert), Path::new(key))?;
            tracing::info!("TLS enabled");
            Some(acceptor)
        }
        _ => None,
    };

    // Initialize optional error pages
    let error_pages = match &config.error_pages_dir {
        Some(dir) => match server::error_pages::ErrorPages::load(Path::new(dir)) {
            Ok(pages) => {
                tracing::info!(dir = dir, "Custom error pages loaded");
                Some(Arc::new(pages))
            }
            Err(e) => {
                tracing::warn!(dir = dir, error = %e, "Failed to load error pages");
                None
            }
        },
        None => None,
    };

    // ── Register built-in event handlers ──

    dispatcher.on(handlers::request_id::RequestIdGenerator);
    dispatcher.on(handlers::metrics::MetricsRequestHandler::new(Arc::clone(
        &metrics,
    )));
    dispatcher.on(handlers::metrics::MetricsResponseHandler::new(Arc::clone(
        &metrics,
    )));
    dispatcher.on(handlers::server_header::ServerHeaderHandler);
    if config.access_log {
        dispatcher.on(handlers::access_log::AccessLogHandler);
    }

    if let Some(ref limiter) = rate_limiter {
        dispatcher.on(handlers::rate_limit::RateLimitHandler::new(Arc::clone(
            limiter,
        )));
        tracing::info!("Rate limit handler registered");
    }
    if let Some(ref pages) = error_pages {
        dispatcher.on(handlers::error_pages::ErrorPagesHandler::new(Arc::clone(
            pages,
        )));
        tracing::info!("Error pages handler registered");
    }

    dispatcher.freeze();
    let dispatcher = Arc::new(dispatcher);
    let plugin_manager = Arc::new(plugin_manager);

    if config.compression {
        tracing::info!("Brotli compression enabled");
    }

    // Spawn rate limiter cleanup thread (daemon — no join needed)
    if let Some(ref limiter) = rate_limiter {
        let limiter_ref = Arc::clone(limiter);
        std::thread::Builder::new()
            .name("rate-cleanup".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(60));
                limiter_ref.cleanup();
            })
            .expect("failed to spawn rate limiter cleanup thread");
    }

    // Spawn internal server on its own thread if configured
    if let Some(ref internal_addr) = config.internal_addr {
        let metrics_ref = Arc::clone(&metrics);
        let config_ref = Arc::clone(&config);
        let executor_ref = Arc::clone(&executor);
        let pm_ref = Arc::clone(&plugin_manager);
        let addr = internal_addr.clone();
        std::thread::Builder::new()
            .name("internal-srv".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("failed to build internal server runtime");
                rt.block_on(async {
                    if let Err(e) = server::internal::run_internal_server(
                        &addr,
                        metrics_ref,
                        config_ref,
                        executor_ref,
                        pm_ref,
                    )
                    .await
                    {
                        tracing::error!(error = %e, "Internal server error");
                    }
                });
            })
            .expect("failed to spawn internal server thread");
    }

    // Parse listen address for SO_REUSEPORT sockets
    let listen_addr: std::net::SocketAddr = config
        .server
        .listen_addr
        .parse()
        .map_err(|e| format!("invalid LISTEN_ADDR '{}': {e}", config.server.listen_addr))?;

    let n_workers = std::env::var("WORKERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| {
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
        });

    let shutdown = Arc::new(AtomicBool::new(false));

    let worker_config = WorkerConfig {
        listen_addr,
        server_config: config.server.clone(),
        executor: Arc::clone(&executor),
        metrics: Arc::clone(&metrics),
        dispatcher,
        tls_acceptor,
        compression_enabled: config.compression,
        shutdown: Arc::clone(&shutdown),
    };

    tracing::info!(workers = n_workers, addr = %listen_addr, "Spawning worker threads");

    let handles = worker::spawn_workers(n_workers, worker_config);

    // Notify plugins that server is ready
    plugin_manager.on_ready_all();

    // Block main thread waiting for shutdown signal (self-pipe trick)
    wait_for_shutdown_signal();

    tracing::info!("Received shutdown signal, shutting down");
    plugin_manager.shutdown_all();
    shutdown.store(true, Ordering::SeqCst);
    executor.shutdown();

    // Join all worker threads
    for h in handles {
        h.join().ok();
    }

    tracing::info!("Server stopped");
    Ok(())
}

/// Block the current thread until SIGINT or SIGTERM is received.
/// Uses a self-pipe: the signal handler writes a byte, main thread reads (blocks).
fn wait_for_shutdown_signal() {
    #[cfg(unix)]
    {
        // Create a pipe: read_fd blocks until signal handler writes
        let (read_fd, write_fd) = {
            let mut fds = [0i32; 2];
            assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe() failed");
            (fds[0], fds[1])
        };

        // Store write_fd for signal handler
        SIGNAL_WRITE_FD.store(write_fd, Ordering::SeqCst);

        unsafe {
            libc::signal(
                libc::SIGINT,
                signal_handler as *const () as libc::sighandler_t,
            );
            libc::signal(
                libc::SIGTERM,
                signal_handler as *const () as libc::sighandler_t,
            );
        }

        // Block until signal writes a byte
        let mut buf = [0u8; 1];
        unsafe {
            libc::read(read_fd, buf.as_mut_ptr().cast(), 1);
            libc::close(read_fd);
            libc::close(write_fd);
        }
    }

    #[cfg(not(unix))]
    {
        // Fallback: poll an atomic flag
        static SIGNALED: AtomicBool = AtomicBool::new(false);
        while !SIGNALED.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    }
}

#[cfg(unix)]
static SIGNAL_WRITE_FD: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

#[cfg(unix)]
extern "C" fn signal_handler(_sig: libc::c_int) {
    let fd = SIGNAL_WRITE_FD.load(Ordering::SeqCst);
    if fd >= 0 {
        let buf = [1u8];
        unsafe {
            libc::write(fd, buf.as_ptr().cast(), 1);
        }
    }
}
