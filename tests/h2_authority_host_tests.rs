//! An HTTP/2 request's `:authority` reaches the application as its `Host`.
//!
//! HTTP/2 carries the target host in the `:authority` pseudo-header, and a
//! client is not expected to send a `host` field next to it. hyper puts the
//! authority in the request URI, so a server that reads the host from the
//! `Host` header finds none: `$_SERVER['HTTP_HOST']` was missing,
//! `SERVER_NAME` fell back to `localhost` and the request object's `host()`
//! was empty. RFC 9113 section 8.3.1 forbids taking the target from `Host`
//! when `:authority` is present, and lets a server refuse a request whose
//! `Host` names something else.
//!
//! Nothing here needs PHP: the executor below hands back the `Host` field
//! lines it was given, which is what the SAPI builds `HTTP_HOST`,
//! `SERVER_NAME`, `SERVER_PORT` and the request object's host from.

mod common;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use http::header::HOST;
use http::Request;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use oxphp::events::EventDispatcher;
use oxphp::executor::{ExecuteResult, ScriptExecutor};
use oxphp::metrics::Metrics;
use oxphp::types::{ScriptRequest, ScriptResponse};

/// Answers with the `Host` field lines it was handed, one per line of the
/// body, and counts the requests it was handed.
struct EchoHostLines(Arc<AtomicUsize>);

impl ScriptExecutor for EchoHostLines {
    fn execute(&self, request: ScriptRequest) -> ExecuteResult {
        request.cancel_state.mark_taken();
        self.0.fetch_add(1, Ordering::SeqCst);
        let lines: Vec<String> = request
            .headers
            .get_all(HOST)
            .iter()
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
            .collect();
        ExecuteResult::Immediate(ScriptResponse {
            body: Bytes::from(lines.join("\n")),
            ..Default::default()
        })
    }

    fn shutdown(&self) {}
}

struct Fixture {
    addr: SocketAddr,
    executed: Arc<AtomicUsize>,
    _dir: tempfile::TempDir,
}

async fn start() -> Fixture {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.php"), "<?php\n").unwrap();
    std::fs::write(dir.path().join("a.txt"), "static\n").unwrap();

    let executed = Arc::new(AtomicUsize::new(0));
    let mut dispatcher = EventDispatcher::new();
    dispatcher.on(oxphp::handlers::request_id::RequestIdGenerator);

    let (addr, _server) = common::start_test_server_with_executor(
        dir.path(),
        &oxphp::config::H2Config::default(),
        None,
        Arc::new(Metrics::new()),
        dispatcher,
        oxphp::server::compression::Levels::default(),
        Arc::new(EchoHostLines(Arc::clone(&executed))),
    )
    .await;
    Fixture {
        addr,
        executed,
        _dir: dir,
    }
}

/// The `Host` lines the executor reported; none for an empty body.
fn delivered(body: &[u8]) -> Vec<String> {
    let text = String::from_utf8(body.to_vec()).unwrap();
    if text.is_empty() {
        return Vec::new();
    }
    text.split('\n').map(str::to_owned).collect()
}

/// One request over h2c with prior knowledge. The authority of `target`
/// becomes the `:authority` pseudo-header, and a `target` with none, such as
/// `/index.php`, is sent without one. Each of `host_lines` is a `host` field
/// of its own in the HEADERS frame. Returns the status and the body.
async fn get_h2(addr: SocketAddr, target: &str, host_lines: &[&str]) -> (u16, Vec<u8>) {
    let tcp = TcpStream::connect(addr).await.unwrap();
    let (send_req, conn) = h2::client::handshake(tcp).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut req = Request::builder()
        .method("GET")
        .uri(target)
        .body(())
        .unwrap();
    for line in host_lines {
        req.headers_mut().append(HOST, line.parse().unwrap());
    }
    let mut send_req = send_req.ready().await.unwrap();
    let (resp_fut, _) = send_req.send_request(req, true).unwrap();
    let resp = resp_fut.await.unwrap();
    let status = resp.status().as_u16();

    let mut body = resp.into_body();
    let mut data = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.unwrap();
        let _ = body.flow_control().release_capacity(chunk.len());
        data.extend_from_slice(&chunk);
    }
    (status, data)
}

/// The usual case: a client that sends `:authority` and no `host` field, as
/// curl does. The authority is the request's one `Host` line.
#[tokio::test]
async fn h2_authority_is_the_host() {
    let fx = start().await;
    let (status, body) = get_h2(fx.addr, "http://a.example:8443/index.php", &[]).await;
    assert_eq!(status, 200);
    assert_eq!(
        delivered(&body),
        ["a.example:8443"],
        "the application must get the :authority as its Host"
    );
}

/// A `host` field naming another authority is refused before anything runs,
/// a static file included, and so is one whose first line agrees and a later
/// one does not: whichever authority the application were given, a proxy in
/// front could have routed on the other.
#[tokio::test]
async fn h2_host_naming_another_authority_is_refused() {
    let fx = start().await;
    for target in ["http://a.example/index.php", "http://a.example/a.txt"] {
        for host_lines in [&["b.example"][..], &["a.example", "b.example"]] {
            let (status, body) = get_h2(fx.addr, target, host_lines).await;
            assert_eq!(
                status,
                400,
                "{target} with host {host_lines:?}: {:?}",
                String::from_utf8_lossy(&body)
            );
        }
    }
    assert_eq!(
        fx.executed.load(Ordering::SeqCst),
        0,
        "a refused request must not reach the application"
    );
}

/// A `host` field naming the same authority is not a conflict, whatever its
/// case, and however many times it is repeated: the application gets one
/// line, spelled as the `:authority` is.
#[tokio::test]
async fn h2_host_naming_the_same_authority_is_replaced_by_it() {
    let fx = start().await;
    let (status, body) = get_h2(
        fx.addr,
        "http://a.example/index.php",
        &["A.Example", "A.Example"],
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(delivered(&body), ["a.example"]);
}

/// A request with no `:authority` keeps the `host` field it came with: there
/// is nothing for it to disagree with. RFC 7540 had a proxy translating an
/// origin-form HTTP/1.1 request send one this way, and so does the h2 client
/// for a target with no authority.
#[tokio::test]
async fn h2_without_authority_keeps_its_host() {
    let fx = start().await;
    let (status, body) = get_h2(fx.addr, "/index.php", &["b.example"]).await;
    assert_eq!(status, 200);
    assert_eq!(delivered(&body), ["b.example"]);
}

/// HTTP/1.1 keeps its `Host` header, even with an absolute-form target that
/// names another authority: RFC 9112 gives that target a rule of its own, and
/// this does not apply it.
#[tokio::test]
async fn h1_host_is_left_as_it_came() {
    let fx = start().await;
    let head = "GET http://a.example/index.php HTTP/1.1\r\n\
                Host: b.example\r\n\
                Connection: close\r\n\r\n";
    let mut sock = TcpStream::connect(fx.addr).await.unwrap();
    sock.write_all(head.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();

    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("no end of response head");
    let status = String::from_utf8_lossy(&buf[..split]);
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    assert_eq!(delivered(&buf[split + 4..]), ["b.example"]);
}
