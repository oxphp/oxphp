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
//! HTTP/1.x has the same rule for a target in absolute form
//! (`GET http://a.example/ HTTP/1.1`): RFC 9112 section 3.2.2 has an origin
//! server ignore the `Host` header and take the host the target names, which
//! hyper also puts in the request URI. Neither protocol counts the userinfo a
//! target may carry before an `@` as part of the host.
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

/// An HTTP/2 `CONNECT` carries `:authority` and no `:scheme` (RFC 9113
/// section 8.5), and the authority is still its host: a `host` field naming
/// another one is refused, though an HTTP/1.x target with no scheme keeps its
/// `Host`.
#[tokio::test]
async fn h2_connect_host_naming_another_authority_is_refused() {
    let fx = start().await;
    let tcp = TcpStream::connect(fx.addr).await.unwrap();
    let (send_req, conn) = h2::client::handshake(tcp).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let req = Request::builder()
        .method("CONNECT")
        .uri("a.example:443")
        .header(HOST, "b.example")
        .body(())
        .unwrap();
    let mut send_req = send_req.ready().await.unwrap();
    let (resp_fut, _) = send_req.send_request(req, true).unwrap();
    assert_eq!(resp_fut.await.unwrap().status().as_u16(), 400);
    assert_eq!(fx.executed.load(Ordering::SeqCst), 0);
}

/// One HTTP/1.x request written to a raw socket as `head`, which the server
/// must close the connection after. Returns the status and the body.
async fn get_h1(addr: SocketAddr, head: &str) -> (u16, Vec<u8>) {
    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(head.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();

    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("no end of response head");
    let status_line = String::from_utf8_lossy(&buf[..split]).into_owned();
    let status = status_line
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .unwrap_or_else(|| panic!("no status in {status_line:?}"));
    (status, buf[split + 4..].to_vec())
}

/// An absolute-form target names the host, and replaces the `Host` header
/// sent with it: RFC 9112 section 3.2.2 has an origin server ignore that
/// header. A request that sent none, as HTTP/1.0 lets it, gets one the same
/// way.
#[tokio::test]
async fn h1_absolute_form_target_is_the_host() {
    let fx = start().await;
    for head in [
        "GET http://a.example:8443/index.php HTTP/1.1\r\n\
         Host: b.example\r\n\
         Connection: close\r\n\r\n",
        "GET http://a.example:8443/index.php HTTP/1.0\r\n\r\n",
    ] {
        let (status, body) = get_h1(fx.addr, head).await;
        assert_eq!(status, 200, "{head:?}");
        assert_eq!(
            delivered(&body),
            ["a.example:8443"],
            "{head:?}: the application must get the target's host as its Host"
        );
    }
}

/// The userinfo an absolute-form target may carry before an `@` is no part of
/// its host, and does not reach the application in `Host`. The host follows
/// the last `@`, where the URI parser looks for it too.
#[tokio::test]
async fn h1_absolute_form_userinfo_is_not_part_of_the_host() {
    let fx = start().await;
    for head in [
        "GET http://u:p@a.example/index.php HTTP/1.1\r\n\
         Host: a.example\r\n\
         Connection: close\r\n\r\n",
        "GET http://u:p@a.example/index.php HTTP/1.0\r\n\r\n",
        "GET http://u@v@a.example/index.php HTTP/1.0\r\n\r\n",
    ] {
        let (status, body) = get_h1(fx.addr, head).await;
        assert_eq!(status, 200, "{head:?}");
        assert_eq!(delivered(&body), ["a.example"], "{head:?}");
    }
}

/// The URI parser finds an authority in a target with no scheme as well: the
/// authority-form one RFC 9112 keeps for `CONNECT`, and one that only lacks
/// its leading `/`. Neither is the absolute form RFC 9112 section 3.2.2
/// speaks of, and the `Host` header stays as it came. Asked with `GET`,
/// because a successful answer to a `CONNECT` carries no body to read it
/// from.
#[tokio::test]
async fn h1_target_without_scheme_keeps_its_host() {
    let fx = start().await;
    for target in ["a.example:80", "index.php"] {
        let head = format!(
            "GET {target} HTTP/1.1\r\n\
             Host: b.example\r\n\
             Connection: close\r\n\r\n"
        );
        let (status, body) = get_h1(fx.addr, &head).await;
        assert_eq!(status, 200, "{target}");
        assert_eq!(delivered(&body), ["b.example"], "{target}");
    }
}

/// An origin-form target names no host, so the `Host` header stays as it
/// came.
#[tokio::test]
async fn h1_origin_form_keeps_its_host() {
    let fx = start().await;
    let (status, body) = get_h1(
        fx.addr,
        "GET /index.php HTTP/1.1\r\n\
         Host: b.example\r\n\
         Connection: close\r\n\r\n",
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(delivered(&body), ["b.example"]);
}

/// Over HTTP/2 the userinfo is no part of the host either, and RFC 9113
/// section 8.3.1 has a client leave it out of an `http` or `https`
/// `:authority`. The application gets the host without it, a `host` field
/// naming that host is no conflict, and a `host` field carrying the userinfo
/// names something else.
#[tokio::test]
async fn h2_authority_userinfo_is_not_part_of_the_host() {
    let fx = start().await;
    let target = "http://u@a.example/index.php";
    for host_lines in [&[][..], &["A.Example"]] {
        let (status, body) = get_h2(fx.addr, target, host_lines).await;
        assert_eq!(status, 200, "host {host_lines:?}");
        assert_eq!(delivered(&body), ["a.example"], "host {host_lines:?}");
    }
    let (status, _) = get_h2(fx.addr, target, &["u@a.example"]).await;
    assert_eq!(status, 400);
    assert_eq!(
        fx.executed.load(Ordering::SeqCst),
        2,
        "the refused request must not reach the application"
    );
}
