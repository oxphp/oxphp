mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::time::Duration;

use oxphp::events::EventDispatcher;

/// Start the server on a random port and return the bound address.
async fn start_server(document_root: &std::path::Path, entry_file: Option<&str>) -> SocketAddr {
    start_server_with_options(document_root, entry_file, None).await
}

/// Start the server with optional rate limiter and return the bound address.
async fn start_server_with_options(
    document_root: &std::path::Path,
    entry_file: Option<&str>,
    rate_limiter: Option<Arc<oxphp::server::rate_limit::RateLimiter>>,
) -> SocketAddr {
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
    dispatcher.on(oxphp::handlers::security_headers::SecurityHeadersHandler::new("DENY"));
    dispatcher.on(oxphp::handlers::access_log::AccessLogHandler::new(
        oxphp::config::AccessLogLevel::All,
    ));
    if let Some(ref limiter) = rate_limiter {
        dispatcher.on(oxphp::handlers::rate_limit::RateLimitHandler::new(
            Arc::clone(limiter),
            Arc::clone(&metrics),
        ));
    }

    let (addr, _server) = common::start_test_server(
        document_root,
        &oxphp::config::H2Config::default(),
        entry_file,
        metrics,
        dispatcher,
    )
    .await;
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
    assert_eq!(id_str.len(), 20);
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
    assert_eq!(server_hdr, "OxPHP");

    let nosniff = resp
        .headers()
        .get("x-content-type-options")
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(nosniff, "nosniff");
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
    // Bind a non-blocking std listener and hand it to the internal server,
    // mirroring how main() pre-binds before any privilege drop.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();

    let metrics = Arc::new(oxphp::metrics::Metrics::new());
    let config = Arc::new(oxphp::config::Config::from_env().unwrap());
    let executor: Arc<dyn oxphp::executor::ScriptExecutor> =
        Arc::new(oxphp::executor::stub::StubExecutor::new());

    let plugin_manager = Arc::new(oxphp::plugin::PluginManager::new());
    tokio::spawn(async move {
        let _ = oxphp::server::internal::run_internal_server(
            listener,
            metrics,
            config,
            executor,
            plugin_manager,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let addr = listener.local_addr().unwrap();

    let metrics = Arc::new(oxphp::metrics::Metrics::new());
    let config = Arc::new(oxphp::config::Config::from_env().unwrap());
    let executor: Arc<dyn oxphp::executor::ScriptExecutor> =
        Arc::new(oxphp::executor::stub::StubExecutor::new());

    let plugin_manager = Arc::new(oxphp::plugin::PluginManager::new());
    tokio::spawn(async move {
        let _ = oxphp::server::internal::run_internal_server(
            listener,
            metrics,
            config,
            executor,
            plugin_manager,
            Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
