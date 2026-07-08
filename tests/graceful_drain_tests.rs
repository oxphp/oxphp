//! Graceful-drain behaviour on shutdown (HTTP layer, no libphp.so needed).
//!
//! When the server begins draining (SIGTERM → `Server::shutdown()`), a
//! well-behaved server tells its live connections to wind down instead of
//! silently holding them until the drain deadline:
//!   * HTTP/2  → a `GOAWAY` frame, so the client finishes in-flight streams
//!     and opens new ones on a fresh connection (a healthy peer).
//!   * HTTP/1.1 → `Connection: close` / socket close on the idle keep-alive
//!     connection.
//!
//! Both tests drive raw sockets against the exact same `auto` builder that
//! production uses, with the stub executor (no PHP dispatch). `Server::shutdown()`
//! flips a per-server drain latch that each connection selects on; on the flip it
//! calls hyper's `graceful_shutdown()`, so an established HTTP/2 connection gets a
//! GOAWAY and an idle HTTP/1.1 keep-alive is closed. These tests assert that
//! observable wind-down; they fail (time out) if the drain latch or the
//! `graceful_shutdown()` wiring regresses.

mod common;

use common::raw;

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use oxphp::config::H2Config;
use oxphp::events::EventDispatcher;
use oxphp::server::Server;

/// Start a cleartext server on an ephemeral port and return its address plus a
/// handle so the test can trigger `shutdown()` while a connection is live.
async fn start_server(document_root: &Path) -> (SocketAddr, Arc<Server>) {
    let mut dispatcher = EventDispatcher::new();
    dispatcher.on(oxphp::handlers::request_id::RequestIdGenerator);
    dispatcher.on(oxphp::handlers::server_header::ServerHeaderHandler);

    common::start_test_server(
        document_root,
        &H2Config::default(),
        None,
        Arc::new(oxphp::metrics::Metrics::new()),
        dispatcher,
    )
    .await
}

#[tokio::test]
async fn http2_connection_receives_goaway_when_server_drains() {
    let dir = tempfile::TempDir::new().unwrap();
    let (addr, server) = start_server(dir.path()).await;

    // h2c prior-knowledge handshake (preface + SETTINGS) via the shared raw
    // frame client. Waiting for the server's SETTINGS — `read_until` ACKs it —
    // both confirms the connection handshook (it is genuinely alive, not dying
    // from a protocol violation on our side) and keeps it open so any
    // subsequent close is attributable to the drain, not to us.
    let mut conn = raw::Conn::connect(addr).await;
    let alive = tokio::time::timeout(
        Duration::from_secs(2),
        conn.read_until(|f| f.ftype == raw::FRAME_SETTINGS),
    )
    .await;
    assert!(
        matches!(alive, Ok(Some(_))),
        "h2 handshake never completed — test precondition failed"
    );

    // Connection is established and healthy. Begin draining.
    server.shutdown();

    let got_goaway = tokio::time::timeout(
        Duration::from_secs(2),
        conn.read_until(|f| f.ftype == raw::FRAME_GOAWAY),
    )
    .await;

    assert!(
        matches!(got_goaway, Ok(Some(_))),
        "server did not send GOAWAY after drain began"
    );
}

#[tokio::test]
async fn http1_keepalive_connection_closes_when_server_drains() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("test.php"), b"<?php echo 'ok';").unwrap();
    let (addr, server) = start_server(dir.path()).await;

    let mut stream = TcpStream::connect(addr).await.unwrap();
    stream
        .write_all(b"GET /test.php HTTP/1.1\r\nHost: localhost\r\nConnection: keep-alive\r\n\r\n")
        .await
        .unwrap();

    // Consume the first response so the connection is an idle keep-alive.
    let mut buf = [0u8; 4096];
    let n = stream.read(&mut buf).await.unwrap();
    assert!(n > 0, "expected a response to the first request");

    // Begin draining.
    server.shutdown();

    // A draining server closes the idle keep-alive connection promptly.
    let closed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match stream.read(&mut buf).await {
                Ok(0) | Err(_) => break, // EOF / reset = connection closed
                Ok(_) => continue,       // trailing bytes — keep reading
            }
        }
    })
    .await;

    assert!(
        closed.is_ok(),
        "keep-alive connection was not closed after drain began"
    );
}
