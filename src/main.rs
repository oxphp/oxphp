#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod logging;

use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::signal;
use tokio::sync::Semaphore;

/// Process-global Tokio runtime handle for async promise await operations.
/// Set once in async_main(), read by PHP worker threads for block_on().
pub static TOKIO_HANDLE: OnceLock<Handle> = OnceLock::new();

use oxphp::config;
use oxphp::events::EventDispatcher;
use oxphp::executor;
use oxphp::handlers;
use oxphp::metrics::Metrics;
use oxphp::plugin::PluginManager;
use oxphp::server;
use oxphp::types;

fn main() -> Result<(), types::BoxError> {
    let mut config = Arc::new(config::Config::from_env()?);

    // Create metrics early — needed by executor for worker metrics
    let metrics = Arc::new(Metrics::new());

    // Initialize plugins BEFORE PHP startup so MINIT can register plugin
    // functions with Zend (OPcache needs them at compile time).
    let mut dispatcher = EventDispatcher::new();
    let mut plugin_manager = PluginManager::new();
    #[cfg(feature = "plugin-example")]
    plugin_manager.add(Box::new(oxphp::plugins::example::ExamplePlugin::new()));
    #[cfg(feature = "plugin-otel")]
    plugin_manager.add(Box::new(oxphp::plugins::otel::OtelPlugin::new()));
    #[cfg(feature = "plugin-apm")]
    plugin_manager.add(Box::new(oxphp::plugins::apm::ApmPlugin::new()));
    plugin_manager.init_all(&mut dispatcher)?;

    // Re-read trace_context after plugin init — the OTel plugin sets
    // TRACE_CONTEXT=true via env::set_var during its init(), but Config
    // was already parsed before plugins ran. Safe to get_mut here because
    // no clones of the Arc exist yet.
    if !config.trace_context
        && std::env::var("TRACE_CONTEXT")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false)
    {
        Arc::get_mut(&mut config)
            .expect("config Arc not yet shared")
            .trace_context = true;
    }

    #[cfg(feature = "php")]
    {
        // Set superglobals flag before PHP startup (read during MINIT and request handling)
        unsafe {
            oxphp::php::bindings::oxphp_bridge_set_superglobals_enabled(
                config.superglobals_enabled,
            );
        }

        // Register request accessor callbacks for the HTTP Object API
        oxphp::php::sapi::register_request_accessors();

        let native_fns = plugin_manager.take_native_php_functions();
        if !native_fns.is_empty() {
            oxphp::php::sapi::register_native_plugin_functions(native_fns);
        }

        // Register plugin PHP definitions (classes, interfaces, enums, attributes, functions)
        let php_defs = plugin_manager.take_php_definitions();
        if !php_defs.classes.is_empty()
            || !php_defs.interfaces.is_empty()
            || !php_defs.enums.is_empty()
            || !php_defs.attributes.is_empty()
            || !php_defs.functions.is_empty()
        {
            oxphp::php::sapi::register_php_definitions(php_defs);
        }
    }

    #[cfg(feature = "php")]
    {
        // Create decorator registry — always, even without Rust plugins,
        // because PHP decorators register at runtime via oxphp_register_decorator()
        let registry = std::sync::Arc::new(oxphp::decorator::DecoratorRegistry::new());
        let decorator_defs = plugin_manager.take_decorators();
        for def in decorator_defs {
            registry.register_rust(std::sync::Arc::from(def.decorator));
        }
        oxphp::decorator::dispatch::install_bridge_callbacks(std::sync::Arc::clone(&registry));
        tracing::info!(
            rust_decorators = registry.rust_decorator_count(),
            "Decorator registry initialized"
        );
    }

    // Create executor AFTER plugin functions are on the bridge —
    // php_module_startup() (MINIT) registers them with Zend.
    let executor: Arc<dyn executor::ScriptExecutor> =
        Arc::from(executor::create_executor(Arc::clone(&metrics)));

    let async_pool = oxphp::executor::async_pool::AsyncWorkerPool::new(
        config.async_workers,
        config.async_queue_capacity,
        Some(Arc::clone(&metrics)),
    );

    let cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let tokio_workers: usize = std::env::var("TOKIO_WORKERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or((cpu / 2).max(1));

    let runtime = if tokio_workers > 1 {
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
        async_pool,
    ))
}

async fn async_main(
    config: Arc<config::Config>,
    executor: Arc<dyn executor::ScriptExecutor>,
    metrics: Arc<Metrics>,
    mut dispatcher: EventDispatcher,
    plugin_manager: PluginManager,
    mut async_pool: Option<executor::async_pool::AsyncWorkerPool>,
) -> Result<(), types::BoxError> {
    TOKIO_HANDLE
        .set(Handle::current())
        .expect("TOKIO_HANDLE already set");

    #[cfg(feature = "php")]
    if let Some(ref mut pool) = async_pool {
        pool.start();
        oxphp::php::sapi::set_global_async_tx(pool.task_sender());
        oxphp::php::sapi::set_async_tokio_handle(Handle::current());
        oxphp::php::sapi::register_async_callbacks();
        oxphp::php::sapi::set_async_metrics(Arc::clone(&metrics));
    }

    // Register fiber scheduler callbacks (try_recv, prepare_request)
    #[cfg(feature = "php")]
    oxphp::php::sapi::register_fiber_callbacks();

    let _log_guard = logging::init(&config.log_level)?;

    let mode = if config.worker_file.is_some() {
        "worker"
    } else {
        match config.server.index_file.as_deref() {
            Some("index.php") => "framework",
            Some("index.html") => "spa",
            _ => "traditional",
        }
    };

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        listen_addr = %config.server.listen_addr,
        document_root = %config.server.document_root.display(),
        executor = %config.executor_type,
        mode = mode,
        "OxPHP HTTP server starting"
    );

    // Start dynamic worker scale manager if configured
    executor.start_scale_manager();

    // Initialize optional rate limiter
    let rate_limiter = if config.rate_limit > 0 {
        let limiter = Arc::new(server::rate_limit::RateLimiter::new(
            config.rate_limit,
            config.rate_window_seconds,
        ));
        tracing::info!(
            rate_limit = config.rate_limit,
            rate_window_seconds = config.rate_window_seconds,
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
    if config.access_log != oxphp::config::AccessLogLevel::Off {
        dispatcher.on(handlers::access_log::AccessLogHandler::new(
            config.access_log,
        ));
    }

    // Conditional handlers
    if let Some(ref limiter) = rate_limiter {
        dispatcher.on(handlers::rate_limit::RateLimitHandler::new(
            Arc::clone(limiter),
            Arc::clone(&metrics),
        ));
        tracing::info!("Rate limit handler registered");
    }
    if let Some(ref pages) = error_pages {
        dispatcher.on(handlers::error_pages::ErrorPagesHandler::new(Arc::clone(
            pages,
        )));
        tracing::info!("Error pages handler registered");
    }

    dispatcher.on(handlers::trace_context::TraceContextRequestHandler::new(
        config.trace_context,
    ));
    dispatcher.on(handlers::trace_context::TraceContextResponseHandler::new(
        config.trace_context,
    ));
    if config.trace_context {
        tracing::info!("Trace context handler registered");
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

    if config.compression_level > 0 {
        tracing::info!(
            level = config.compression_level,
            "Brotli compression enabled"
        );
    } else {
        tracing::info!("Brotli compression disabled");
    }

    if !config.static_cache_enabled {
        tracing::info!("Static file content cache: mtime revalidation (STATIC_CACHE=off)");
    }

    if config.server.split_path_info {
        tracing::info!("PATH_INFO splitting enabled (SPLIT_PATH_INFO_ENABLED=true)");
    }

    let server = Arc::new(server::Server::new(
        &config.server,
        executor,
        Arc::clone(&metrics),
        dispatcher,
        tls_acceptor,
        config.compression_level,
        config.max_query_body,
        config.worker_file.clone(),
        config
            .static_cache_ttl
            .map(|ttl| format!("public, max-age={ttl}")),
        config.static_cache_enabled,
    ));
    let semaphore = Arc::new(Semaphore::new(config.max_connections));

    // Notify plugins that server is ready
    plugin_manager.on_ready_all();

    // Spawn graceful shutdown handler
    let shutdown_notify = Arc::new(tokio::sync::Notify::new());
    let server_ref = Arc::clone(&server);
    let pm_shutdown = Arc::clone(&plugin_manager);
    let shutdown_ref = Arc::clone(&shutdown_notify);
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("Received shutdown signal, draining connections");
        pm_shutdown.shutdown_all();
        server_ref.shutdown();
        shutdown_ref.notify_one();
    });

    // Accept loop
    loop {
        if server.is_shutdown() {
            break;
        }

        let accept_result = tokio::select! {
            result = listener.accept() => result,
            _ = shutdown_notify.notified() => break,
        };

        let (stream, remote_addr) = match accept_result {
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
                let msg = e.to_string();
                if msg.contains("timeout") {
                    tracing::warn!(
                        remote_addr = %remote_addr,
                        error = %e,
                        "Connection timeout"
                    );
                } else {
                    tracing::error!(
                        remote_addr = %remote_addr,
                        error = %e,
                        "Connection error"
                    );
                }
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
        let drain_deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(config.drain_timeout_seconds);
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

    // Shutdown async worker pool
    if let Some(ref mut pool) = async_pool {
        pool.shutdown();
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
