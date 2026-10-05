//! The request's `User-Agent` reaches `RequestComplete`, which is where the
//! access log, the trace span and the profiler's run record are written.
//!
//! The headers are handed to the executor along with the rest of the request,
//! so whatever the completion side needs from them has to be taken out first —
//! on the path that dispatches the request and on the one a `RequestReceived`
//! handler answers early.
//!
//! Nothing here needs PHP: the stub executor answers the script.

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use oxphp::events::{EventDispatcher, EventHandler, Propagation, RequestComplete, RequestReceived};
use oxphp::metrics::Metrics;
use oxphp::types::full_body;

/// What `RequestComplete` carried for the one request a test sends: `None`
/// until the event fires, then the `User-Agent` it held, if any. Every value
/// these tests send is ASCII.
type Seen = Arc<Mutex<Option<Option<String>>>>;

struct CompleteProbe(Seen);

impl EventHandler<RequestComplete> for CompleteProbe {
    fn handle(&self, event: &mut RequestComplete) -> Propagation {
        *self.0.lock().unwrap() = Some(
            event
                .user_agent
                .as_ref()
                .map(|v| String::from_utf8(v.as_bytes().to_vec()).unwrap()),
        );
        Propagation::Continue
    }
}

/// Answers every request itself, before it is dispatched, the way the rate
/// limiter answers a `429`.
struct AnswerEarly;

impl EventHandler<RequestReceived> for AnswerEarly {
    fn handle(&self, event: &mut RequestReceived) -> Propagation {
        event.early_response = Some(
            http::Response::builder()
                .status(429)
                .body(full_body(bytes::Bytes::from_static(b"slow down")))
                .unwrap(),
        );
        Propagation::Continue
    }
}

async fn start(document_root: &std::path::Path, seen: &Seen, early: bool) -> SocketAddr {
    let mut dispatcher = EventDispatcher::new();
    if early {
        dispatcher.on(AnswerEarly);
    }
    dispatcher.on(CompleteProbe(Arc::clone(seen)));
    let (addr, _server) = common::start_test_server(
        document_root,
        &oxphp::config::H2Config::default(),
        None,
        Arc::new(Metrics::new()),
        dispatcher,
    )
    .await;
    addr
}

/// Sends one request with these extra header lines and returns its status
/// line once the whole response has been read — after `RequestComplete`,
/// which fires before the response is handed back.
async fn send(addr: SocketAddr, extra_headers: &[u8]) -> String {
    let mut sock = TcpStream::connect(addr).await.unwrap();
    let mut request =
        b"GET /index.php HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n".to_vec();
    request.extend_from_slice(extra_headers);
    request.extend_from_slice(b"\r\n");
    sock.write_all(&request).await.unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();
    String::from_utf8_lossy(&buf)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

async fn user_agent_seen(extra_headers: &[u8], early: bool) -> (String, Option<String>) {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.php"), "<?php\n").unwrap();
    let seen: Seen = Arc::default();
    let addr = start(dir.path(), &seen, early).await;
    let status = send(addr, extra_headers).await;
    let carried = seen
        .lock()
        .unwrap()
        .take()
        .expect("RequestComplete did not fire for the request");
    (status, carried)
}

/// A request that is dispatched — its headers go to the executor with it.
#[tokio::test]
async fn a_dispatched_request_completes_with_its_user_agent() {
    let (status, carried) = user_agent_seen(b"User-Agent: test-bot/1.0\r\n", false).await;
    assert_eq!(status, "HTTP/1.1 200 OK");
    assert_eq!(carried.as_deref(), Some("test-bot/1.0"));
}

/// A request a `RequestReceived` handler answered never reaches the executor
/// and completes from its own construction site.
#[tokio::test]
async fn a_request_answered_early_completes_with_its_user_agent() {
    let (status, carried) = user_agent_seen(b"User-Agent: test-bot/1.0\r\n", true).await;
    assert_eq!(status, "HTTP/1.1 429 Too Many Requests");
    assert_eq!(carried.as_deref(), Some("test-bot/1.0"));
}

/// The control for both: a request without the header completes with none,
/// on either path, so the two above are not reading a value from somewhere
/// else.
#[tokio::test]
async fn a_request_without_a_user_agent_completes_with_none() {
    for early in [false, true] {
        let (_, carried) = user_agent_seen(b"", early).await;
        assert_eq!(carried, None, "early answer: {early}");
    }
}

/// Of two `User-Agent` field lines the first is the one carried — the line a
/// plugin reading the request's headers sees too.
#[tokio::test]
async fn of_two_user_agent_lines_the_first_is_carried() {
    let (_, carried) = user_agent_seen(
        b"User-Agent: first/1.0\r\nUser-Agent: second/2.0\r\n",
        false,
    )
    .await;
    assert_eq!(carried.as_deref(), Some("first/1.0"));
}
