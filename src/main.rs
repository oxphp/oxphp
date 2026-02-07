mod logging;

use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::signal;

use oxphp::config;
use oxphp::server;
use oxphp::types;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), types::BoxError> {
    let config = config::Config::from_env()?;

    logging::init(&config.log_level)?;

    tracing::info!(
        listen_addr = %config.server.listen_addr,
        document_root = %config.server.document_root.display(),
        "OxPHP HTTP server starting"
    );

    let listener = TcpListener::bind(&config.server.listen_addr).await?;
    let local_addr = listener.local_addr()?;

    tracing::info!(addr = %local_addr, "Server listening");

    let server = Arc::new(server::Server::new(&config.server));

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

        let server_clone = Arc::clone(&server);
        tokio::spawn(async move {
            if let Err(e) = server_clone.handle_connection(stream, remote_addr).await {
                tracing::error!(
                    remote_addr = %remote_addr,
                    error = %e,
                    "Connection error"
                );
            }
        });
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
