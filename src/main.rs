mod logging;

use std::path::Path;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;
use tokio::sync::Semaphore;

use oxphp::config;
use oxphp::executor;
use oxphp::metrics::Metrics;
use oxphp::server;
use oxphp::types;

fn main() -> Result<(), types::BoxError> {
    let config = config::Config::from_env()?;

    // Create executor BEFORE Tokio runtime — PHP TSRM init must happen
    // on the main thread before any async runtime signal handling.
    let executor: std::sync::Arc<dyn executor::ScriptExecutor> =
        std::sync::Arc::from(executor::create_executor());

    let mut builder = tokio::runtime::Builder::new_multi_thread();
    if config.worker_threads > 0 {
        builder.worker_threads(config.worker_threads);
    }
    builder.enable_all();

    let runtime = builder.build()?;
    runtime.block_on(async_main(config, executor))
}

async fn async_main(
    config: config::Config,
    executor: Arc<dyn executor::ScriptExecutor>,
) -> Result<(), types::BoxError> {
    let _log_guard = logging::init(&config.log_level)?;

    tracing::info!(
        listen_addr = %config.server.listen_addr,
        document_root = %config.server.document_root.display(),
        worker_threads = config.worker_threads,
        executor = %config.executor_type,
        "OxPHP HTTP server starting"
    );

    // Initialize metrics
    let metrics = Arc::new(Metrics::new(config.worker_threads));

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

    let listener = TcpListener::bind(&config.server.listen_addr).await?;
    let local_addr = listener.local_addr()?;

    tracing::info!(addr = %local_addr, "Server listening");

    // Spawn internal server if configured (before Server::new consumes executor)
    let internal_handle = if let Some(ref internal_addr) = config.internal_addr {
        let metrics_ref = Arc::clone(&metrics);
        let config_ref = Arc::new(config::Config::from_env()?);
        let executor_ref = Arc::clone(&executor);
        let addr = internal_addr.clone();
        Some(tokio::spawn(async move {
            if let Err(e) =
                server::internal::run_internal_server(&addr, metrics_ref, config_ref, executor_ref)
                    .await
            {
                tracing::error!(error = %e, "Internal server error");
            }
        }))
    } else {
        None
    };

    let server = Arc::new(server::Server::new(
        &config.server,
        executor,
        Arc::clone(&metrics),
        rate_limiter,
        tls_acceptor,
        error_pages,
    ));
    let semaphore = Arc::new(Semaphore::new(config.max_connections));

    // Spawn graceful shutdown handler
    let server_ref = Arc::clone(&server);
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("Received shutdown signal, draining connections");
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
