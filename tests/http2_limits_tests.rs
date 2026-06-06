//! Integration tests for HTTP/2 connection-level abuse limits (`H2Config`).
//!
//! HTTP/2 is served on both the TLS path (negotiated via ALPN) and the
//! cleartext path (hyper's `auto` builder detects the HTTP/2 client preface).
//! These tests drive the cleartext h2c path with prior knowledge, so they need
//! no certificates while still exercising the exact same `.http2()` builder that
//! TLS connections use. The stub executor (no libphp.so) is sufficient: every
//! limit lives in the HTTP/2 framing layer, below PHP dispatch.
//!
//! Two clients are used:
//! * the high-level `h2` crate, for well-behaved sessions (AC1, AC4 control,
//!   and the non-starved connection in AC3);
//! * a minimal hand-rolled frame client (`raw` module below), for the abusive
//!   cases. The `h2` client is RFC-compliant — it queues streams that would
//!   exceed the peer's advertised `MAX_CONCURRENT_STREAMS` and never floods —
//!   so it cannot provoke the server's `REFUSED_STREAM` / `GOAWAY` enforcement.
//!   The raw client deliberately ignores the advertised limits to do so.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use h2::client::{self, SendRequest};
use http::Request;
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use oxphp::config::{H2Config, ServerConfig};
use oxphp::events::EventDispatcher;

/// Start a cleartext server with a custom `H2Config` on an ephemeral port.
async fn start_h2_server(document_root: &Path, h2: H2Config) -> SocketAddr {
    let config = ServerConfig::new("127.0.0.1:0".to_string(), document_root.to_path_buf());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let executor: Arc<dyn oxphp::executor::ScriptExecutor> =
        Arc::new(oxphp::executor::stub::StubExecutor::new());
    let metrics = Arc::new(oxphp::metrics::Metrics::new());

    let mut dispatcher = EventDispatcher::new();
    dispatcher.on(oxphp::handlers::request_id::RequestIdGenerator);
    dispatcher.on(oxphp::handlers::metrics::MetricsRequestHandler::new(
        Arc::clone(&metrics),
    ));
    dispatcher.on(oxphp::handlers::metrics::MetricsResponseHandler::new(
        Arc::clone(&metrics),
    ));
    dispatcher.on(oxphp::handlers::server_header::ServerHeaderHandler);
    dispatcher.freeze();

    let server = Arc::new(oxphp::server::Server::new(
        &config,
        &h2,
        executor,
        metrics,
        Arc::new(dispatcher),
        None,                                      // no TLS — cleartext h2c
        0,                                         // compression disabled
        512 * 1024,                                // max_query_body
        None,                                      // entry_file
        false,                                     // worker_mode_enabled
        Some("public, max-age=86400".to_string()), // static_cache_control
        false,                                     // static_revalidate
        Arc::new(AtomicBool::new(false)),          // shutdown flag
    ));

    tokio::spawn(async move {
        loop {
            let (stream, remote_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };
            let server = Arc::clone(&server);
            tokio::spawn(async move {
                let _ = server.handle_connection(stream, remote_addr).await;
            });
        }
    });

    addr
}

/// Open an h2c connection (prior knowledge) and spawn its connection driver.
async fn connect_h2(addr: SocketAddr) -> (SendRequest<Bytes>, tokio::task::JoinHandle<()>) {
    let tcp = TcpStream::connect(addr).await.unwrap();
    let (send_req, conn) = client::handshake(tcp).await.unwrap();
    let handle = tokio::spawn(async move {
        let _ = conn.await;
    });
    (send_req, handle)
}

/// Drain a response body to bytes, releasing flow-control capacity as it goes.
async fn read_body(mut body: h2::RecvStream) -> Vec<u8> {
    let mut data = Vec::new();
    while let Some(chunk) = body.data().await {
        let chunk = chunk.unwrap();
        let _ = body.flow_control().release_capacity(chunk.len());
        data.extend_from_slice(&chunk);
    }
    data
}

/// AC1: with default limits, a normal h2 session that opens several concurrent
/// streams (a browser fetching multiple assets over one connection) is fully
/// served — the limits do not interfere with legitimate multiplexing.
#[tokio::test]
async fn test_h2_concurrent_assets_served_under_default_limits() {
    let dir = tempfile::TempDir::new().unwrap();
    for i in 0..6 {
        std::fs::write(
            dir.path().join(format!("asset{i}.css")),
            format!("body{{--n:{i}}}"),
        )
        .unwrap();
    }

    let addr = start_h2_server(dir.path(), H2Config::default()).await;
    let (mut send_req, _conn) = connect_h2(addr).await;

    // Fire all six HEADERS before reading any response → genuinely concurrent
    // open streams (well under the default cap of 32).
    let mut pending = Vec::new();
    for i in 0..6 {
        let req = Request::builder()
            .method("GET")
            .uri(format!("http://{addr}/asset{i}.css"))
            .body(())
            .unwrap();
        send_req = send_req.ready().await.unwrap();
        let (resp_fut, _send) = send_req.send_request(req, true).unwrap();
        pending.push((i, resp_fut));
    }

    for (i, resp_fut) in pending {
        let resp = resp_fut.await.unwrap();
        assert_eq!(resp.status(), 200, "asset{i} should be served");
        let body = read_body(resp.into_body()).await;
        assert_eq!(body, format!("body{{--n:{i}}}").into_bytes());
    }
}

/// Wiring guard: the configured `max_concurrent_streams` and `max_header_list_bytes`
/// are advertised verbatim in the server's initial SETTINGS frame. This fails
/// deterministically if the `.http2()` builder ever stops applying the config
/// (e.g. a knob is dropped and the value silently reverts to a library default).
/// Distinctive non-default values are used so a default cannot accidentally match.
#[tokio::test]
async fn test_h2_server_advertises_configured_limits() {
    let dir = tempfile::TempDir::new().unwrap();
    let h2 = H2Config {
        max_concurrent_streams: 7,
        max_header_list_bytes: 9000,
        ..H2Config::default()
    };
    let addr = start_h2_server(dir.path(), h2).await;

    let mut conn = raw::Conn::connect(addr).await;
    let settings = conn.read_server_settings().await;
    let get = |id: u16| settings.iter().find(|(i, _)| *i == id).map(|(_, v)| *v);

    assert_eq!(
        get(raw::SETTINGS_MAX_CONCURRENT_STREAMS),
        Some(7),
        "server must advertise the configured max_concurrent_streams"
    );
    assert_eq!(
        get(raw::SETTINGS_MAX_HEADER_LIST_SIZE),
        Some(9000),
        "server must advertise the configured max_header_list_bytes"
    );
}

/// AC2: `max_concurrent_streams` is enforced against a misbehaving client. With
/// a cap of 2, a client that opens a third concurrent stream (ignoring the
/// advertised limit) gets `REFUSED_STREAM` on the excess stream rather than
/// having it queued into the worker pool.
///
/// Streams 1 and 3 are held open by POSTing to a PHP route without a body
/// (`dispatch_request` awaits the full body before dispatch), so they still
/// count against the cap when stream 5 arrives.
#[tokio::test]
async fn test_h2_excess_streams_refused() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("warm.css"), "warm").unwrap();
    std::fs::write(dir.path().join("hold.php"), "<?php echo 'x';").unwrap();

    let h2 = H2Config {
        max_concurrent_streams: 2,
        ..H2Config::default()
    };
    let addr = start_h2_server(dir.path(), h2).await;
    let mut conn = raw::Conn::connect(addr).await;

    // Hold two streams open (POST, no END_STREAM → server blocks awaiting body).
    conn.send_request(1, raw::METHOD_POST, "/hold.php", false)
        .await
        .unwrap();
    conn.send_request(3, raw::METHOD_POST, "/hold.php", false)
        .await
        .unwrap();
    // Third concurrent stream exceeds the advertised cap of 2.
    conn.send_request(5, raw::METHOD_GET, "/warm.css", true)
        .await
        .unwrap();

    let frame = timeout(
        Duration::from_secs(3),
        conn.read_until(|f| {
            (f.ftype == raw::FRAME_RST_STREAM && f.stream_id == 5) || f.ftype == raw::FRAME_GOAWAY
        }),
    )
    .await
    .expect("server must respond to the excess stream within the timeout")
    .expect("connection closed before rejecting the excess stream");

    assert_eq!(
        frame.ftype,
        raw::FRAME_RST_STREAM,
        "excess stream should be a stream error (RST_STREAM), not a connection error (GOAWAY)"
    );
    assert_eq!(frame.stream_id, 5);
    assert_eq!(
        raw::error_code(&frame),
        raw::REFUSED_STREAM,
        "excess stream over max_concurrent_streams must be REFUSED_STREAM"
    );
}

/// AC3: a Rapid-Reset burst (open + immediate `RST_STREAM`, CVE-2023-44487) is
/// bounded and does not starve other connections. A raw client floods one
/// connection with open+reset pairs faster than the server can accept them;
/// once it exceeds `max_pending_accept_reset` the server closes that connection
/// with `GOAWAY (ENHANCE_YOUR_CALM)`. Throughout, a separate well-behaved
/// connection keeps being served.
#[tokio::test]
async fn test_h2_rapid_reset_flood_is_bounded_and_does_not_starve() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("asset.css"), "asset").unwrap();

    let h2 = H2Config {
        max_pending_accept_reset: 10,
        ..H2Config::default()
    };
    let addr = start_h2_server(dir.path(), h2).await;

    // Connection A: raw rapid-reset flood. Returns the GOAWAY error code, if any.
    let flood = tokio::spawn(async move {
        let mut conn = raw::Conn::connect(addr).await;
        let mut sid = 1u32;
        for _ in 0..200 {
            if conn
                .send_request(sid, raw::METHOD_GET, "/asset.css", false)
                .await
                .is_err()
            {
                break;
            }
            if conn.send_rst(sid, raw::CANCEL).await.is_err() {
                break;
            }
            sid += 2;
        }
        match timeout(
            Duration::from_secs(5),
            conn.read_until(|f| f.ftype == raw::FRAME_GOAWAY),
        )
        .await
        {
            Ok(Some(f)) => Some(raw::error_code(&f)),
            _ => None,
        }
    });

    // Connection B: a normal client must keep being served during the flood.
    let (mut send_req, _conn) = connect_h2(addr).await;
    for _ in 0..20 {
        let req = Request::builder()
            .method("GET")
            .uri(format!("http://{addr}/asset.css"))
            .body(())
            .unwrap();
        send_req = send_req.ready().await.unwrap();
        let (resp_fut, _s) = send_req.send_request(req, true).unwrap();
        let resp = timeout(Duration::from_secs(5), resp_fut)
            .await
            .expect("normal connection must not be starved by a rapid-reset flood")
            .expect("normal request should not error during a flood");
        assert_eq!(resp.status(), 200);
        let _ = read_body(resp.into_body()).await;
    }

    let goaway = flood.await.unwrap();
    assert_eq!(
        goaway,
        Some(raw::ENHANCE_YOUR_CALM),
        "a rapid-reset flood exceeding max_pending_accept_reset must be GOAWAY'd with ENHANCE_YOUR_CALM"
    );
}

/// AC4: a HEADERS block whose decoded size exceeds `max_header_list_bytes` is
/// rejected — either the compliant client refuses to send headers larger than
/// the peer's advertised limit, or the server resets the stream — never a 200.
/// A normal request under the cap is unaffected.
#[tokio::test]
async fn test_h2_oversized_header_list_rejected() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("ok.css"), "ok").unwrap();

    let h2 = H2Config {
        max_header_list_bytes: 8 * 1024,
        ..H2Config::default()
    };
    let addr = start_h2_server(dir.path(), h2).await;

    // Control: a normal, small request under the cap is served.
    {
        let (mut send_req, _conn) = connect_h2(addr).await;
        let req = Request::builder()
            .method("GET")
            .uri(format!("http://{addr}/ok.css"))
            .body(())
            .unwrap();
        send_req = send_req.ready().await.unwrap();
        let (resp_fut, _s) = send_req.send_request(req, true).unwrap();
        assert_eq!(resp_fut.await.unwrap().status(), 200);
    }

    // A single ~16 KiB header value blows past the 8 KiB decoded-header cap.
    let (mut send_req, _conn) = connect_h2(addr).await;
    let big = "a".repeat(16 * 1024);
    let req = Request::builder()
        .method("GET")
        .uri(format!("http://{addr}/ok.css"))
        .header("x-big", big)
        .body(())
        .unwrap();
    send_req = send_req.ready().await.unwrap();
    match send_req.send_request(req, true) {
        // Client refused to send headers exceeding the peer's advertised limit.
        Err(_) => {}
        // ...or the server rejected the stream after receiving them.
        Ok((resp_fut, _s)) => {
            let result = resp_fut.await;
            let served_ok = result.as_ref().map(|r| r.status() == 200).unwrap_or(false);
            assert!(
                !served_ok,
                "oversized HEADERS must not be served as 200 (got {result:?})"
            );
        }
    }
}

/// A minimal hand-rolled HTTP/2 client: just enough framing and static-table
/// HPACK to open streams, reset them, and parse the server's control frames.
/// Used only for the abusive cases the RFC-compliant `h2` client refuses to do.
mod raw {
    use std::net::SocketAddr;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

    // Frame types (RFC 9113 §6).
    const FRAME_HEADERS: u8 = 0x1;
    pub const FRAME_RST_STREAM: u8 = 0x3;
    const FRAME_SETTINGS: u8 = 0x4;
    pub const FRAME_GOAWAY: u8 = 0x7;

    // Frame flags.
    const FLAG_END_STREAM: u8 = 0x1;
    const FLAG_END_HEADERS: u8 = 0x4;
    const FLAG_ACK: u8 = 0x1;

    // SETTINGS parameter identifiers.
    pub const SETTINGS_MAX_CONCURRENT_STREAMS: u16 = 0x3;
    pub const SETTINGS_MAX_HEADER_LIST_SIZE: u16 = 0x6;

    // Error codes (RFC 9113 §7).
    pub const REFUSED_STREAM: u32 = 0x7;
    pub const CANCEL: u32 = 0x8;
    pub const ENHANCE_YOUR_CALM: u32 = 0xb;

    // HPACK static-table fully-indexed pseudo-headers (RFC 7541 Appendix A).
    pub const METHOD_GET: u8 = 0x82; // :method: GET   (index 2)
    pub const METHOD_POST: u8 = 0x83; // :method: POST  (index 3)
    const SCHEME_HTTP: u8 = 0x86; // :scheme: http  (index 6)
    const NAME_PATH: u8 = 0x04; // literal w/o indexing, name index 4 (:path)
    const NAME_AUTHORITY: u8 = 0x01; // literal w/o indexing, name index 1 (:authority)

    pub struct Frame {
        pub ftype: u8,
        pub flags: u8,
        pub stream_id: u32,
        pub payload: Vec<u8>,
    }

    pub struct Conn {
        sock: TcpStream,
        authority: String,
    }

    impl Conn {
        pub async fn connect(addr: SocketAddr) -> Self {
            let mut sock = TcpStream::connect(addr).await.unwrap();
            sock.write_all(PREFACE).await.unwrap();
            write_frame(&mut sock, FRAME_SETTINGS, 0, 0, &[])
                .await
                .unwrap();
            sock.flush().await.unwrap();
            Self {
                sock,
                authority: addr.to_string(),
            }
        }

        /// Send a request HEADERS frame. `method` is a fully-indexed HPACK byte
        /// (`METHOD_GET` / `METHOD_POST`); `path` and the authority are encoded
        /// as literals without indexing (no dynamic-table mutation, no Huffman).
        pub async fn send_request(
            &mut self,
            stream_id: u32,
            method: u8,
            path: &str,
            end_stream: bool,
        ) -> std::io::Result<()> {
            assert!(path.len() < 127 && self.authority.len() < 127);
            let mut block = vec![method, SCHEME_HTTP, NAME_PATH, path.len() as u8];
            block.extend_from_slice(path.as_bytes());
            block.push(NAME_AUTHORITY);
            block.push(self.authority.len() as u8);
            block.extend_from_slice(self.authority.as_bytes());

            let mut flags = FLAG_END_HEADERS;
            if end_stream {
                flags |= FLAG_END_STREAM;
            }
            write_frame(&mut self.sock, FRAME_HEADERS, flags, stream_id, &block).await?;
            self.sock.flush().await
        }

        pub async fn send_rst(&mut self, stream_id: u32, error: u32) -> std::io::Result<()> {
            write_frame(
                &mut self.sock,
                FRAME_RST_STREAM,
                0,
                stream_id,
                &error.to_be_bytes(),
            )
            .await?;
            self.sock.flush().await
        }

        async fn read_frame(&mut self) -> std::io::Result<Frame> {
            let mut hdr = [0u8; 9];
            self.sock.read_exact(&mut hdr).await?;
            let len = ((hdr[0] as usize) << 16) | ((hdr[1] as usize) << 8) | hdr[2] as usize;
            let stream_id = u32::from_be_bytes([hdr[5] & 0x7f, hdr[6], hdr[7], hdr[8]]);
            let mut payload = vec![0u8; len];
            self.sock.read_exact(&mut payload).await?;
            Ok(Frame {
                ftype: hdr[3],
                flags: hdr[4],
                stream_id,
                payload,
            })
        }

        /// Read frames, auto-ACKing the server's SETTINGS, until `pred` matches
        /// (returns that frame) or the connection ends (returns `None`).
        pub async fn read_until<F: Fn(&Frame) -> bool>(&mut self, pred: F) -> Option<Frame> {
            loop {
                let frame = self.read_frame().await.ok()?;
                if frame.ftype == FRAME_SETTINGS && frame.flags & FLAG_ACK == 0 {
                    let _ = write_frame(&mut self.sock, FRAME_SETTINGS, FLAG_ACK, 0, &[]).await;
                    let _ = self.sock.flush().await;
                }
                if pred(&frame) {
                    return Some(frame);
                }
            }
        }

        /// Read the server's initial (non-ACK) SETTINGS frame as (id, value) pairs.
        pub async fn read_server_settings(&mut self) -> Vec<(u16, u32)> {
            loop {
                let frame = self.read_frame().await.unwrap();
                if frame.ftype == FRAME_SETTINGS && frame.flags & FLAG_ACK == 0 {
                    return frame
                        .payload
                        .chunks(6)
                        .map(|c| {
                            (
                                u16::from_be_bytes([c[0], c[1]]),
                                u32::from_be_bytes([c[2], c[3], c[4], c[5]]),
                            )
                        })
                        .collect();
                }
            }
        }
    }

    /// Error code from an RST_STREAM (bytes 0..4) or GOAWAY (bytes 4..8) payload.
    pub fn error_code(frame: &Frame) -> u32 {
        let off = if frame.ftype == FRAME_GOAWAY { 4 } else { 0 };
        u32::from_be_bytes([
            frame.payload[off],
            frame.payload[off + 1],
            frame.payload[off + 2],
            frame.payload[off + 3],
        ])
    }

    async fn write_frame(
        sock: &mut TcpStream,
        ftype: u8,
        flags: u8,
        stream_id: u32,
        payload: &[u8],
    ) -> std::io::Result<()> {
        let len = payload.len();
        let hdr = [
            (len >> 16) as u8,
            (len >> 8) as u8,
            len as u8,
            ftype,
            flags,
            (stream_id >> 24) as u8,
            (stream_id >> 16) as u8,
            (stream_id >> 8) as u8,
            stream_id as u8,
        ];
        sock.write_all(&hdr).await?;
        sock.write_all(payload).await
    }
}
