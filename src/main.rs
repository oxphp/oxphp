#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod logging;

use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::Semaphore;

use oxphp::config;
use oxphp::events::EventDispatcher;
use oxphp::executor;
use oxphp::handlers;
use oxphp::metrics::Metrics;
use oxphp::plugin::PluginManager;
use oxphp::server;
use oxphp::types;

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

    // Create executor AFTER plugin functions are on the bridge —
    // php_module_startup() (MINIT) registers them with Zend.
    let executor: Arc<dyn executor::ScriptExecutor> =
        Arc::from(executor::create_executor(Arc::clone(&metrics)));

    let tokio_workers: usize = std::env::var("TOKIO_WORKERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0);

    let runtime = if tokio_workers > 0 {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(tokio_workers)
            .enable_all()
            .build()?
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
    };
    runtime.block_on(async_main(
        config,
        executor,
        metrics,
        dispatcher,
        plugin_manager,
    ))
}

async fn async_main(
    config: Arc<config::Config>,
    executor: Arc<dyn executor::ScriptExecutor>,
    metrics: Arc<Metrics>,
    mut dispatcher: EventDispatcher,
    plugin_manager: PluginManager,
) -> Result<(), types::BoxError> {
    let _log_guard = logging::init(&config.log_level)?;

    tracing::info!(
        listen_addr = %config.server.listen_addr,
        document_root = %config.server.document_root.display(),
        executor = %config.executor_type,
        "OxPHP HTTP server starting"
    );

    // Start dynamic worker scale manager if configured
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

    let listener = TcpListener::bind(&config.server.listen_addr).await?;
    let local_addr = listener.local_addr()?;

    tracing::info!(addr = %local_addr, "Server listening");

    // Spawn internal server if configured (before Server::new consumes executor)
    let internal_handle = if let Some(ref internal_addr) = config.internal_addr {
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

    let server = Arc::new(server::Server::new(
        &config.server,
        executor,
        Arc::clone(&metrics),
        dispatcher,
        tls_acceptor,
        config.compression,
        config.max_query_body,
    ));
    let semaphore = Arc::new(Semaphore::new(config.max_connections));

    // Notify plugins that server is ready
    plugin_manager.on_ready_all();

    // Spawn graceful shutdown handler
    let server_ref = Arc::clone(&server);
    let pm_shutdown = Arc::clone(&plugin_manager);
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("Received shutdown signal, draining connections");
        pm_shutdown.shutdown_all();
        server_ref.shutdown();
    });

    // Accept loop
    loop {
        if server.is_shutdown() {
            break;
        }

        let (stream, remote_addr) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!(error = %e, "Failed to accept connection");
                continue;
            }
        };

        let permit = match semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(_) => break, // semaphore closed — shutting down
        };

        let server_clone = Arc::clone(&server);
        tokio::spawn(async move {
            let _permit = permit; // held until task completes
            if let Err(e) = server_clone.handle_connection(stream, remote_addr).await {
                tracing::error!(
                    remote_addr = %remote_addr,
                    error = %e,
                    "Connection error"
                );
            }
        });
    }

    // Graceful drain: wait for in-flight connections to finish
    let active = server.active_connections();
    if active > 0 {
        tracing::info!(
            active_connections = active,
            "Draining in-flight connections"
        );
        let drain_deadline =
            tokio::time::Instant::now() + std::time::Duration::from_secs(config.drain_timeout_secs);
        loop {
            let remaining = server.active_connections();
            if remaining == 0 {
                tracing::info!("All connections drained");
                break;
            }
            if tokio::time::Instant::now() >= drain_deadline {
                tracing::warn!(
                    remaining_connections = remaining,
                    "Drain timeout reached, forcing shutdown"
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
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
