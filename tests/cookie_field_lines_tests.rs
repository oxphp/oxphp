//! A `Cookie` header that arrives as several field lines reaches the plugins
//! and the application as one.
//!
//! HTTP/2 lets a client split its cookies across field lines for better HPACK
//! compression, and RFC 9113 section 8.2.3 makes the server join them with
//! `"; "` before handing them to an application. Over HTTP/1.1 a user agent
//! must not send more than one (RFC 6265 section 5.4), but one that does is
//! handled the same way. Readers that take the first line, the last or every
//! line otherwise see different cookies, and the stripping of `__oxp_*`
//! cookies takes the first.
//!
//! Nothing here needs PHP: the executor below hands back the `Cookie` field
//! lines it was given, which is what the SAPI builds `$_COOKIE` and
//! `$_SERVER['HTTP_COOKIE']` from.

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http::header::COOKIE;
use http::Request;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

use oxphp::events::{EventDispatcher, EventHandler, Propagation, RequestReceived};
use oxphp::executor::{ExecuteResult, ScriptExecutor};
use oxphp::metrics::Metrics;
use oxphp::plugin::cookies::extract_plugin_cookies;
use oxphp::types::{ScriptRequest, ScriptResponse};

/// What a plugin handler saw of the `Cookie` header during `RequestReceived`.
#[derive(Default, Debug, PartialEq)]
struct PluginSaw {
    lines: usize,
    /// The `s` cookie of a plugin whose prefix is `__oxp_t_`.
    secret: Option<String>,
}

struct CookieProbe(Arc<Mutex<PluginSaw>>);

impl EventHandler<RequestReceived> for CookieProbe {
    fn handle(&self, event: &mut RequestReceived) -> Propagation {
        let headers = &event.parts.headers;
        *self.0.lock().unwrap() = PluginSaw {
            lines: headers.get_all(COOKIE).iter().count(),
            secret: extract_plugin_cookies(headers, "__oxp_t_")
                .get("s")
                .map(str::to_owned),
        };
        Propagation::Continue
    }
}

/// Answers with the number of `Cookie` field lines it was handed, then each of
/// them, one per line of the body.
struct EchoCookieLines;

impl ScriptExecutor for EchoCookieLines {
    fn execute(&self, request: ScriptRequest) -> ExecuteResult {
        request.cancel_state.mark_taken();
        let lines: Vec<String> = request
            .headers
            .get_all(COOKIE)
            .iter()
            .map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned())
            .collect();
        let mut body = lines.len().to_string();
        for line in &lines {
            body.push('\n');
            body.push_str(line);
        }
        ExecuteResult::Immediate(ScriptResponse {
            body: Bytes::from(body),
            ..Default::default()
        })
    }

    fn shutdown(&self) {}
}

/// The `Cookie` field lines the executor reported, in order.
fn delivered(body: &[u8]) -> Vec<String> {
    let text = String::from_utf8(body.to_vec()).unwrap();
    let mut lines = text.split('\n');
    let count: usize = lines.next().unwrap().parse().unwrap();
    let lines: Vec<String> = lines.map(str::to_owned).collect();
    assert_eq!(lines.len(), count, "malformed echo: {text:?}");
    lines
}

struct Fixture {
    addr: SocketAddr,
    plugin_saw: Arc<Mutex<PluginSaw>>,
    _dir: tempfile::TempDir,
}

async fn start() -> Fixture {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.php"), "<?php\n").unwrap();

    let plugin_saw = Arc::new(Mutex::new(PluginSaw::default()));
    let mut dispatcher = EventDispatcher::new();
    dispatcher.on(oxphp::handlers::request_id::RequestIdGenerator);
    dispatcher.on(CookieProbe(Arc::clone(&plugin_saw)));

    let (addr, _server) = common::start_test_server_with_executor(
        dir.path(),
        &oxphp::config::H2Config::default(),
        None,
        Arc::new(Metrics::new()),
        dispatcher,
        oxphp::server::compression::Levels::default(),
        Arc::new(EchoCookieLines),
    )
    .await;
    Fixture {
        addr,
        plugin_saw,
        _dir: dir,
    }
}

/// One HTTP/1.1 request carrying each of `cookie_lines` as its own `Cookie`
/// header; returns the response body.
async fn get_h1(addr: SocketAddr, cookie_lines: &[&str]) -> Vec<u8> {
    let mut head = String::from("GET /index.php HTTP/1.1\r\nHost: localhost\r\n");
    for line in cookie_lines {
        head.push_str(&format!("Cookie: {line}\r\n"));
    }
    head.push_str("Connection: close\r\n\r\n");

    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(head.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    sock.read_to_end(&mut buf).await.unwrap();

    let split = buf
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("no end of response head");
    let status = String::from_utf8_lossy(&buf[..split]);
    assert!(status.starts_with("HTTP/1.1 200"), "{status}");
    buf[split + 4..].to_vec()
}

/// The same over h2c with prior knowledge: each of `cookie_lines` is its own
/// `cookie` field in the HEADERS frame.
async fn get_h2(addr: SocketAddr, cookie_lines: &[&str]) -> Vec<u8> {
    let tcp = TcpStream::connect(addr).await.unwrap();
    let (send_req, conn) = h2::client::handshake(tcp).await.unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let mut req = Request::builder()
        .method("GET")
        .uri(format!("http://{addr}/index.php"))
        .body(())
        .unwrap();
    for line in cookie_lines {
        req.headers_mut().append(COOKIE, line.parse().unwrap());
    }
    let mut send_req = send_req.ready().await.unwrap();
    let (resp_fut, _) = send_req.send_request(req, true).unwrap();
    let resp = resp_fut.await.unwrap();
    assert_eq!(resp.status(), 200);

    let mut body = resp.into_body();
    let mut data = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.unwrap();
        let _ = body.flow_control().release_capacity(chunk.len());
        data.extend_from_slice(&chunk);
    }
    data
}

#[derive(Clone, Copy)]
enum Proto {
    H1,
    H2,
}

async fn send(fx: &Fixture, proto: Proto, cookie_lines: &[&str]) -> Vec<String> {
    let body = match proto {
        Proto::H1 => get_h1(fx.addr, cookie_lines).await,
        Proto::H2 => get_h2(fx.addr, cookie_lines).await,
    };
    delivered(&body)
}

/// A plugin cookie on a later field line is stripped like one on the first.
/// Left on its own line, it reached the application through
/// `$_SERVER['HTTP_COOKIE']` and the request object's `headers()`, which took
/// the last line.
async fn plugin_cookie_on_a_later_line_is_stripped(proto: Proto) {
    let fx = start().await;
    let lines = send(&fx, proto, &["a=1", "__oxp_t_s=1; b=2"]).await;
    assert_eq!(
        lines,
        ["a=1; b=2"],
        "the application must get one Cookie line holding every application cookie and no plugin one"
    );
}

/// A plugin cookie on the first field line costs nothing on the others. The
/// stripping used to rewrite the header from the first line alone, dropping
/// every later one with it.
async fn application_cookies_after_a_plugin_cookie_survive(proto: Proto) {
    let fx = start().await;
    let lines = send(&fx, proto, &["__oxp_t_s=1", "a=1; b=2"]).await;
    assert_eq!(
        lines,
        ["a=1; b=2"],
        "the application cookies on the second line must reach the application"
    );
}

/// Plugins see the joined header too: a plugin's cookie on a later field line
/// is one it can read, because the lines are joined before `RequestReceived`.
async fn plugins_see_one_joined_line(proto: Proto) {
    let fx = start().await;
    send(&fx, proto, &["a=1", "__oxp_t_s=1; b=2"]).await;
    assert_eq!(
        *fx.plugin_saw.lock().unwrap(),
        PluginSaw {
            lines: 1,
            secret: Some("1".to_owned()),
        },
        "plugins must see one Cookie line and their own cookie on it"
    );
}

#[tokio::test]
async fn h1_plugin_cookie_on_a_later_line_is_stripped() {
    plugin_cookie_on_a_later_line_is_stripped(Proto::H1).await;
}

#[tokio::test]
async fn h2_plugin_cookie_on_a_later_line_is_stripped() {
    plugin_cookie_on_a_later_line_is_stripped(Proto::H2).await;
}

#[tokio::test]
async fn h1_application_cookies_after_a_plugin_cookie_survive() {
    application_cookies_after_a_plugin_cookie_survive(Proto::H1).await;
}

#[tokio::test]
async fn h2_application_cookies_after_a_plugin_cookie_survive() {
    application_cookies_after_a_plugin_cookie_survive(Proto::H2).await;
}

#[tokio::test]
async fn h1_plugins_see_one_joined_line() {
    plugins_see_one_joined_line(Proto::H1).await;
}

#[tokio::test]
async fn h2_plugins_see_one_joined_line() {
    plugins_see_one_joined_line(Proto::H2).await;
}

/// The control: a single field line passes through untouched, plugin cookies
/// aside.
#[tokio::test]
async fn a_single_line_is_left_as_it_came() {
    let fx = start().await;
    assert_eq!(send(&fx, Proto::H1, &["a=1; b=2"]).await, ["a=1; b=2"]);
    assert_eq!(send(&fx, Proto::H2, &["a=1; b=2"]).await, ["a=1; b=2"]);
    assert_eq!(
        send(&fx, Proto::H1, &["a=1; __oxp_t_s=1; b=2"]).await,
        ["a=1; b=2"]
    );
}
