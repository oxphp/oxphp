use std::net::SocketAddr;
use std::sync::Arc;

use tokio::net::TcpListener;

/// Start the server on a random port and return the bound address.
async fn start_server(document_root: &std::path::Path, index_file: Option<&str>) -> SocketAddr {
    let config = oxphp::config::ServerConfig::new(
        "127.0.0.1:0".to_string(),
        document_root.to_path_buf(),
        index_file.map(String::from),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let executor: Arc<dyn oxphp::executor::ScriptExecutor> =
        Arc::new(oxphp::executor::stub::StubExecutor::new());
    let server = Arc::new(oxphp::server::Server::new(&config, executor));

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
