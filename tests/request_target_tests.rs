//! A request target in none of the forms HTTP gives its method is refused,
//! and so is an absolute URI with no host; an absolute-form target with no
//! path reaches the application with the path `/`.
//!
//! RFC 9112 section 3.2 gives an HTTP/1.x request target four forms: a path
//! starting with `/`, an absolute URI, the `host:port` of a `CONNECT` and the
//! `*` of a server-wide `OPTIONS`. The URI parser reads a target with neither
//! a leading `/` nor a `scheme://` as the third, whatever the method, and
//! `GET index.php` was served as a request for `/`, as was `GET mailto:u@x`,
//! an absolute URI with no host, which no `http` or `https` URI is; `*` was
//! served with any method. HTTP/2 carries the target in `:path`, which RFC
//! 9113 section 8.3.1 has start with `/` unless it is the `*` of an
//! `OPTIONS`, and the HTTP/2 layer does not look at how it starts, nor, with
//! no `:authority`, whether there is one. Every one of these is answered with
//! `400` before anything runs. A `:path` holding only a fragment arrives
//! empty, and with an `:authority` it is served as a request for `/`.
//!
//! An absolute-form target may have an empty path, as `http://a.example?x=1`
//! has, where the same request in origin form carries `/`. `REQUEST_URI` was
//! `?x=1`, while the router and the request object's `path()` read `/`.
//!
//! Nothing here needs PHP: the executor below answers with the path and query
//! the SAPI builds `REQUEST_URI` from.

mod common;

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use http::{Method, Request, Uri};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use oxphp::events::EventDispatcher;
use oxphp::executor::{ExecuteResult, ScriptExecutor};
use oxphp::metrics::Metrics;
use oxphp::types::{ScriptRequest, ScriptResponse};

/// The body of the answer to a target in none of the forms.
const REFUSED: &str = "400 Bad Request: invalid request target";

/// Answers with the path and query it was handed, read the way the SAPI reads
/// them into `REQUEST_URI`, and counts the requests it was handed.
struct EchoTarget(Arc<AtomicUsize>);

impl ScriptExecutor for EchoTarget {
    fn execute(&self, request: ScriptRequest) -> ExecuteResult {
        request.cancel_state.mark_taken();
        self.0.fetch_add(1, Ordering::SeqCst);
        let target = request
            .uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");
        ExecuteResult::Immediate(ScriptResponse {
            body: Bytes::from(target.to_owned()),
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
        Arc::new(EchoTarget(Arc::clone(&executed))),
    )
    .await;
    Fixture {
        addr,
        executed,
        _dir: dir,
    }
}

/// One HTTP/1.x request written to a raw socket as `head`, which the server
/// must close the connection after. Returns the status and the body.
async fn get_h1(addr: SocketAddr, head: &str) -> (u16, String) {
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
    (
        status,
        String::from_utf8_lossy(&buf[split + 4..]).into_owned(),
    )
}

/// One HTTP/1.1 request for `target` with `method`, closing the connection.
async fn request_h1(addr: SocketAddr, method: &str, target: &str) -> (u16, String) {
    let head = format!(
        "{method} {target} HTTP/1.1\r\n\
         Host: a.example\r\n\
         Connection: close\r\n\r\n"
    );
    get_h1(addr, &head).await
}

/// One request over h2c with prior knowledge. The `:path` is whatever the
/// path and query of `path` are, which the URI builder takes without a leading
/// `/`, and the `:authority` is `a.example`. Returns the status and the body.
async fn request_h2(addr: SocketAddr, method: Method, path: &str) -> (u16, String) {
    let tcp = TcpStream::connect(addr).await.unwrap();
    let (send_req, conn) = h2::client::handshake(tcp).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let uri = Uri::builder()
        .scheme("http")
        .authority("a.example")
        .path_and_query(path)
        .build()
        .unwrap();
    let req = Request::builder().method(method).uri(uri).body(()).unwrap();
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
    (status, String::from_utf8_lossy(&data).into_owned())
}

/// One HTTP/2 frame: the 9-byte header and `payload`.
fn h2_frame(kind: u8, flags: u8, stream: u32, payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u32;
    let mut frame = vec![(len >> 16) as u8, (len >> 8) as u8, len as u8, kind, flags];
    frame.extend_from_slice(&stream.to_be_bytes());
    frame.extend_from_slice(payload);
    frame
}

/// One request over h2c with prior knowledge whose header block is `fields`
/// and nothing else, for what the HTTP/2 client does not send: a `:path` left
/// out, or one the URI type would not keep. Returns the body of the answer.
async fn request_h2_raw(addr: SocketAddr, fields: &[(&str, &str)]) -> String {
    // HPACK literal fields without indexing, every name and value under 127
    // bytes, so each length is one byte.
    let mut block = Vec::new();
    for (name, value) in fields {
        block.push(0);
        block.push(name.len() as u8);
        block.extend_from_slice(name.as_bytes());
        block.push(value.len() as u8);
        block.extend_from_slice(value.as_bytes());
    }
    let mut sock = TcpStream::connect(addr).await.unwrap();
    let mut out = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n".to_vec();
    out.extend(h2_frame(0x4, 0, 0, &[]));
    // HEADERS with END_STREAM and END_HEADERS.
    out.extend(h2_frame(0x1, 0x1 | 0x4, 1, &block));
    sock.write_all(&out).await.unwrap();

    let mut body = Vec::new();
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let mut head = [0u8; 9];
            sock.read_exact(&mut head).await.unwrap();
            let len = u32::from_be_bytes([0, head[0], head[1], head[2]]) as usize;
            let (kind, flags) = (head[3], head[4]);
            let stream = u32::from_be_bytes([head[5], head[6], head[7], head[8]]) & 0x7fff_ffff;
            let mut payload = vec![0u8; len];
            sock.read_exact(&mut payload).await.unwrap();
            match (kind, stream) {
                (0x4, 0) if flags & 0x1 == 0 => {
                    sock.write_all(&h2_frame(0x4, 0x1, 0, &[])).await.unwrap();
                }
                (0x0, 1) => {
                    body.extend_from_slice(&payload);
                    if flags & 0x1 != 0 {
                        return;
                    }
                }
                (0x1, 1) if flags & 0x1 != 0 => return,
                (0x3, 1) | (0x7, 0) => panic!("stream reset or connection closed: {payload:?}"),
                _ => {}
            }
        }
    })
    .await
    .expect("no answer on stream 1");
    String::from_utf8_lossy(&body).into_owned()
}

/// A target with neither a leading `/` nor a `scheme://` is the authority
/// form, which RFC 9112 section 3.2.3 keeps for `CONNECT`, an absolute URI with
/// no host, as `mailto:u@x` is, or none of the forms at all. Each is refused
/// before routing, where it used to be served as a request for `/`. HTTP/1.0
/// gets the same.
#[tokio::test]
async fn h1_target_without_a_leading_slash_or_scheme_slashes_is_refused() {
    let fx = start().await;
    let mut answers = Vec::new();
    for target in [
        "index.php",
        "a.example:80",
        "mailto:u@x",
        ":80",
        "u@:80",
        ":",
    ] {
        let (status, body) = request_h1(fx.addr, "GET", target).await;
        answers.push((target, status, body));
    }
    let (status, body) = get_h1(fx.addr, "GET index.php HTTP/1.0\r\n\r\n").await;
    answers.push(("index.php over HTTP/1.0", status, body));

    let expected: Vec<_> = answers
        .iter()
        .map(|(target, ..)| (*target, 400, REFUSED.to_owned()))
        .collect();
    assert_eq!(
        (answers, fx.executed.load(Ordering::SeqCst)),
        (expected, 0),
        "([(target, status, body)], requests the application ran)"
    );
}

/// `*` names the server as a whole, and RFC 9112 section 3.2.4 and RFC 9113
/// section 8.3.1 keep it for `OPTIONS`. With any other method it is refused,
/// over HTTP/1.1 and HTTP/2, where it used to reach the application.
#[tokio::test]
async fn asterisk_target_is_refused_for_a_method_other_than_options() {
    let fx = start().await;
    let refused = (400, REFUSED.to_owned());
    let answers = [
        request_h1(fx.addr, "GET", "*").await,
        request_h1(fx.addr, "DELETE", "*").await,
        request_h2(fx.addr, Method::GET, "*").await,
    ];
    assert_eq!(
        (answers, fx.executed.load(Ordering::SeqCst)),
        ([refused.clone(), refused.clone(), refused], 0),
        "([GET * over h1, DELETE * over h1, GET * over h2], requests the application ran)"
    );
}

/// An `OPTIONS *` is the one request `*` is for, and it is served as before:
/// it reaches the application with `*` as its target.
#[tokio::test]
async fn options_asterisk_is_served_as_before() {
    let fx = start().await;
    let answers = [
        request_h1(fx.addr, "OPTIONS", "*").await,
        request_h2(fx.addr, Method::OPTIONS, "*").await,
    ];
    let served = (200, "*".to_owned());
    assert_eq!(
        (answers, fx.executed.load(Ordering::SeqCst)),
        ([served.clone(), served], 2),
        "([OPTIONS * over h1, OPTIONS * over h2], requests the application ran)"
    );
}

/// An absolute-form target with an empty path reaches the application with
/// the path `/` in front of its query, the way the same request arrives in
/// origin form, where RFC 9112 section 3.2.1 has the client send that `/`.
/// A target that has a path, in either form, arrives as it was sent.
#[tokio::test]
async fn h1_absolute_form_target_with_no_path_gets_slash() {
    let fx = start().await;
    let mut answers = Vec::new();
    for target in [
        "http://a.example?x=1",
        "http://a.example:8080?x=1",
        "http://a.example",
        "http://a.example/index.php?y=1",
        "/index.php?y=1",
    ] {
        let (status, body) = request_h1(fx.addr, "GET", target).await;
        answers.push((target, status, body));
    }
    let (status, body) = get_h1(fx.addr, "GET http://a.example?x=1 HTTP/1.0\r\n\r\n").await;
    answers.push(("http://a.example?x=1 over HTTP/1.0", status, body));

    assert_eq!(
        answers,
        [
            ("http://a.example?x=1", 200, "/?x=1".to_owned()),
            ("http://a.example:8080?x=1", 200, "/?x=1".to_owned()),
            ("http://a.example", 200, "/".to_owned()),
            (
                "http://a.example/index.php?y=1",
                200,
                "/index.php?y=1".to_owned()
            ),
            ("/index.php?y=1", 200, "/index.php?y=1".to_owned()),
            (
                "http://a.example?x=1 over HTTP/1.0",
                200,
                "/?x=1".to_owned()
            ),
        ],
        "[(target, status, the path and query the application got)]"
    );
}

/// An HTTP/2 `:path` without a leading `/` is malformed by RFC 9113 section
/// 8.3.1, even where the same characters would make a valid absolute-form
/// HTTP/1.1 target, as `?x=1` after an authority does. It is refused, where it
/// used to reach the application as it was sent. One holding only a fragment
/// arrives empty, which the next test covers.
#[tokio::test]
async fn h2_path_without_a_leading_slash_is_refused() {
    let fx = start().await;
    let refused = (400, REFUSED.to_owned());
    let answers = [
        request_h2(fx.addr, Method::GET, "index.php").await,
        request_h2(fx.addr, Method::GET, "?x=1").await,
        request_h2(fx.addr, Method::GET, "/index.php?x=1").await,
    ];
    assert_eq!(
        (answers, fx.executed.load(Ordering::SeqCst)),
        (
            [refused.clone(), refused, (200, "/index.php?x=1".to_owned())],
            1
        ),
        "([:path index.php, :path ?x=1, :path /index.php?x=1], requests the application ran)"
    );
}

/// The HTTP/2 layer lets a request with no `:authority` leave `:path` out,
/// and the URI parser drops a fragment, so a `:path` of `#x` arrives empty.
/// Without an `:authority` either leaves a URI with no path, which used to be
/// routed as `/`, and is refused. With an `:authority`, `#x` reads as `/`
/// everywhere and is served as a request for it.
#[tokio::test]
async fn h2_request_with_no_path_is_refused() {
    let fx = start().await;
    let get = [(":method", "GET"), (":scheme", "http")];
    let host = ("host", "a.example");
    let answers = [
        request_h2_raw(fx.addr, &[get[0], get[1], (":path", "#x"), host]).await,
        request_h2_raw(fx.addr, &[get[0], get[1], host]).await,
        request_h2_raw(
            fx.addr,
            &[get[0], get[1], (":path", "/index.php?x=1"), host],
        )
        .await,
        request_h2_raw(
            fx.addr,
            &[get[0], get[1], (":authority", "a.example"), (":path", "#x")],
        )
        .await,
    ];
    assert_eq!(
        (answers, fx.executed.load(Ordering::SeqCst)),
        (
            [
                REFUSED.to_owned(),
                REFUSED.to_owned(),
                "/index.php?x=1".to_owned(),
                "/".to_owned(),
            ],
            2
        ),
        "([:path #x, no :path, :path /index.php?x=1, :path #x with :authority], \
         requests the application ran)"
    );
}
