#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod logging;

use std::path::Path;
use std::sync::Arc;
use tokio::signal;

use oxphp::config;
use oxphp::events::EventDispatcher;
use oxphp::executor;
use oxphp::handlers;
use oxphp::metrics::Metrics;
use oxphp::plugin::PluginManager;
use oxphp::server;
use oxphp::types;
use oxphp::worker;

fn main() -> Result<(), types::BoxError> {
    let config = Arc::new(config::Config::from_env()?);

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

    // Initialize PHP engine on main thread before spawning workers.
    // Workers call php_thread_init() + execute_request() directly — no channel.
    #[cfg(feature = "php")]
    if config.executor_type == "sapi" {
        oxphp::executor::sapi::php_module_init();
    }

    // Internal server uses a stub executor — worker threads handle PHP directly.
    // SapiExecutor is no longer needed; php_module_init() handles engine startup.
    let executor: Arc<dyn executor::ScriptExecutor> = Arc::new(executor::stub::StubExecutor::new());

    // Build a lightweight Tokio runtime for the main thread (internal server + signals).
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    #[cfg(feature = "php")]
    let is_sapi = config.executor_type == "sapi";

    let result = runtime.block_on(async_main(
        config,
        executor,
        metrics,
        dispatcher,
        plugin_manager,
    ));

    // Shut down PHP engine after all workers have exited.
    #[cfg(feature = "php")]
    if is_sapi {
        oxphp::executor::sapi::php_module_shutdown();
    }

    result
}

async fn async_main(
    config: Arc<config::Config>,
    executor: Arc<dyn executor::ScriptExecutor>,
    metrics: Arc<Metrics>,
    mut dispatcher: EventDispatcher,
    plugin_manager: PluginManager,
) -> Result<(), types::BoxError> {
    let _log_guard = logging::init(&config.log_level)?;

    let num_workers = worker::parse_worker_threads();

    tracing::info!(
        listen_addr = %config.server.listen_addr,
        document_root = %config.server.document_root.display(),
        executor = %config.executor_type,
        workers = num_workers,
        "OxPHP HTTP server starting (thread-per-core)"
    );

    // Start dynamic worker scale manager if configured (SAPI executor)
    executor.start_scale_manager();

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
        // Spawn background cleanup task
        let limiter_ref = Arc::clone(&limiter);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                limiter_ref.cleanup();
            }
        });
        Some(limiter)
    } else {
        None
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

    // Always registered handlers
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

    // Conditional handlers
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

    // Build RouteConfig and FileCache for workers
    let route_config = Arc::new(server::routing::RouteConfig::new(&config.server));
    let file_cache = Arc::new(server::response::static_file::FileCache::new(200));

    // Spawn internal server if configured (hyper-based, unchanged)
    let internal_handle = if let Some(ref internal_addr) = config.internal_addr {
        // Initialize optional TLS for internal server
        let tls_acceptor = match (&config.tls_cert, &config.tls_key) {
            (Some(cert), Some(key)) => {
                let acceptor = server::tls::load_tls_config(Path::new(cert), Path::new(key))?;
                tracing::info!("TLS enabled");
                Some(acceptor)
            }
            _ => None,
        };

        // Build a Server for the internal endpoint (uses hyper)
        let internal_server = Arc::new(server::Server::new(
            &config.server,
            Arc::clone(&executor),
            Arc::clone(&metrics),
            Arc::clone(&dispatcher),
            tls_acceptor,
            config.compression,
        ));
        let _ = internal_server; // kept alive via Arc in internal server

        let metrics_ref = Arc::clone(&metrics);
        let config_ref = Arc::clone(&config);
        let executor_ref = Arc::clone(&executor);
        let pm_ref = Arc::clone(&plugin_manager);
        let addr = internal_addr.clone();
        Some(tokio::spawn(async move {
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
        }))
    } else {
        None
    };

    if config.compression {
        tracing::info!("Brotli compression enabled");
    }

    // Build shared state for worker threads (no executor — workers execute PHP directly)
    let mode = if config.executor_type == "sapi" {
        worker::ExecutorMode::Sapi
    } else {
        worker::ExecutorMode::Stub
    };
    let shared_state = Arc::new(worker::SharedState::new(
        route_config,
        file_cache,
        Arc::clone(&metrics),
        Arc::clone(&dispatcher),
        config.server.request_timeout,
        config.server.header_read_timeout,
        config.compression,
        mode,
    ));

    // Parse listen address
    let listen_addr: std::net::SocketAddr = config.server.listen_addr.parse()?;

    // Spawn worker threads (each with SO_REUSEPORT listener)
    let worker_handles = worker::spawn_workers(Arc::clone(&shared_state), listen_addr, num_workers);

    tracing::info!(addr = %listen_addr, workers = num_workers, "Server listening");

    // Notify plugins that server is ready
    plugin_manager.on_ready_all();

    // Wait for shutdown signal
    shutdown_signal().await;
    tracing::info!("Received shutdown signal, draining connections");

    // Signal shutdown
    plugin_manager.shutdown_all();
    executor.shutdown();
    shared_state.shutdown();

    // Give workers time to drain
    let drain_deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(config.drain_timeout_secs);

    // Wait for active connections to finish
    loop {
        let active = metrics.active_connections();
        if active == 0 {
            tracing::info!("All connections drained");
            break;
        }
        if std::time::Instant::now() >= drain_deadline {
            tracing::warn!(
                remaining_connections = active,
                "Drain timeout reached, forcing shutdown"
            );
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }

    // Join worker threads (with timeout — don't block forever)
    for handle in worker_handles {
        let _ = handle.join();
    }

    // Abort internal server task
    if let Some(handle) = internal_handle {
        handle.abort();
    }

    tracing::info!("Server stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}
