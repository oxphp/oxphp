use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::time::Duration;

use oxphp::events::EventDispatcher;

/// Start the server on a random port and return the bound address.
async fn start_server(document_root: &std::path::Path, index_file: Option<&str>) -> SocketAddr {
    start_server_with_options(document_root, index_file, None).await
}

/// Start the server with optional rate limiter and return the bound address.
async fn start_server_with_options(
    document_root: &std::path::Path,
    index_file: Option<&str>,
    rate_limiter: Option<Arc<oxphp::server::rate_limit::RateLimiter>>,
) -> SocketAddr {
    let config = oxphp::config::ServerConfig::new(
        "127.0.0.1:0".to_string(),
        document_root.to_path_buf(),
        index_file.map(String::from),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let executor: Arc<dyn oxphp::executor::ScriptExecutor> =
        Arc::new(oxphp::executor::stub::StubExecutor::new());
    let metrics = Arc::new(oxphp::metrics::Metrics::new());

    // Build dispatcher with standard handlers
    let mut dispatcher = EventDispatcher::new();
    dispatcher.on(oxphp::handlers::request_id::RequestIdGenerator);
    dispatcher.on(oxphp::handlers::metrics::MetricsRequestHandler::new(
        Arc::clone(&metrics),
    ));
    dispatcher.on(oxphp::handlers::metrics::MetricsResponseHandler::new(
        Arc::clone(&metrics),
    ));
    dispatcher.on(oxphp::handlers::server_header::ServerHeaderHandler);
    dispatcher.on(oxphp::handlers::access_log::AccessLogHandler);
    if let Some(ref limiter) = rate_limiter {
        dispatcher.on(oxphp::handlers::rate_limit::RateLimitHandler::new(
            Arc::clone(limiter),
        ));
    }
    dispatcher.freeze();

    let server = Arc::new(oxphp::server::Server::new(
        &config,
        executor,
        metrics,
        Arc::new(dispatcher),
        None,
        false, // compression disabled in tests
    ));

    tokio::spawn(async move {
        loop {
            let (stream, remote_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };
            let server_clone = Arc::clone(&server);
            tokio::spawn(async move {
                let _ = server_clone.handle_connection(stream, remote_addr).await;
            });
        }
    });

    addr
}

#[tokio::test]
async fn test_static_file_serving() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html>Hello</html>").unwrap();

    let addr = start_server(dir.path(), None).await;
    let url = format!("http://{}/", addr);

    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("Hello"));
}

#[tokio::test]
async fn test_css_content_type() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("style.css"), "body { color: red; }").unwrap();

    let addr = start_server(dir.path(), None).await;
    let url = format!("http://{}/style.css", addr);

    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get("content-type")
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("text/css"));
}

#[tokio::test]
async fn test_not_found() {
    let dir = tempfile::TempDir::new().unwrap();

    let addr = start_server(dir.path(), None).await;
    let url = format!("http://{}/nonexistent.txt", addr);

    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_path_traversal() {
    let dir = tempfile::TempDir::new().unwrap();

    let addr = start_server(dir.path(), None).await;
    let url = format!("http://{}/../../etc/passwd", addr);

    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_request_id_header() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html>Hello</html>").unwrap();

    let addr = start_server(dir.path(), None).await;
    let url = format!("http://{}/", addr);

    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), 200);
    let request_id = resp.headers().get("x-request-id");
    assert!(
        request_id.is_some(),
        "Response should have x-request-id header"
    );
    let id_str = request_id.unwrap().to_str().unwrap();
    assert_eq!(id_str.len(), 16);
    assert!(id_str.chars().all(|c| c.is_ascii_hexdigit()));
}

#[tokio::test]
async fn test_request_id_passthrough() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html>Hello</html>").unwrap();

    let addr = start_server(dir.path(), None).await;
    let url = format!("http://{}/", addr);

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .header("x-request-id", "custom-id-123")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let request_id = resp
        .headers()
        .get("x-request-id")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(request_id, "custom-id-123");
}

#[tokio::test]
async fn test_server_header() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html>Hello</html>").unwrap();

    let addr = start_server(dir.path(), None).await;
    let url = format!("http://{}/", addr);

    let resp = reqwest::get(&url).await.unwrap();
    let server_hdr = resp.headers().get("server").unwrap().to_str().unwrap();
    assert!(server_hdr.starts_with("OxPHP/"));
}

#[tokio::test]
async fn test_rate_limiting_429() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html>Hello</html>").unwrap();

    let limiter = Arc::new(oxphp::server::rate_limit::RateLimiter::new(3, 60));
    let addr = start_server_with_options(dir.path(), None, Some(limiter)).await;
    let url = format!("http://{}/", addr);

    // First 3 requests should succeed
    for _ in 0..3 {
        let resp = reqwest::get(&url).await.unwrap();
        assert_eq!(resp.status(), 200);
    }

    // 4th request should be rate limited
    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), 429);
    assert!(resp.headers().contains_key("retry-after"));
    assert!(resp.headers().contains_key("x-ratelimit-limit"));
    assert!(resp.headers().contains_key("x-ratelimit-remaining"));
}

#[tokio::test]
async fn test_internal_server_health() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener); // free the port for internal server

    let metrics = Arc::new(oxphp::metrics::Metrics::new());
    let config = Arc::new(oxphp::config::Config::from_env().unwrap());
    let executor: Arc<dyn oxphp::executor::ScriptExecutor> =
        Arc::new(oxphp::executor::stub::StubExecutor::new());

    let plugin_manager = Arc::new(oxphp::plugin::PluginManager::new());
    let addr_str = addr.to_string();
    tokio::spawn(async move {
        let _ = oxphp::server::internal::run_internal_server(
            &addr_str,
            metrics,
            config,
            executor,
            plugin_manager,
        )
        .await;
    });

    // Give internal server time to start
    tokio::time::sleep(Duration::from_millis(50)).await;

    let url = format!("http://{}/health", addr);
    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["executor_healthy"], true);
}

#[tokio::test]
async fn test_internal_server_metrics() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let metrics = Arc::new(oxphp::metrics::Metrics::new());
    let config = Arc::new(oxphp::config::Config::from_env().unwrap());
    let executor: Arc<dyn oxphp::executor::ScriptExecutor> =
        Arc::new(oxphp::executor::stub::StubExecutor::new());

    let plugin_manager = Arc::new(oxphp::plugin::PluginManager::new());
    let addr_str = addr.to_string();
    tokio::spawn(async move {
        let _ = oxphp::server::internal::run_internal_server(
            &addr_str,
            metrics,
            config,
            executor,
            plugin_manager,
        )
        .await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let url = format!("http://{}/metrics", addr);
    let resp = reqwest::get(&url).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("oxphp_requests_total"));
    assert!(body.contains("oxphp_busy_workers"));
}
