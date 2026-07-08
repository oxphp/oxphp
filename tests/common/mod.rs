//! Shared bootstrap for integration tests that drive a real `Server` on an
//! ephemeral port with the stub executor (no libphp.so needed).
//!
//! Each test file builds its own `EventDispatcher` (handler sets differ per
//! suite); this module owns the parts that were previously copy-pasted —
//! the many-argument `Server::new` call and the accept loop.

// Not every test binary uses every helper.
#![allow(dead_code)]

use std::net::SocketAddr;
use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tokio::net::TcpListener;

use oxphp::config::{H2Config, ServerConfig};
use oxphp::events::EventDispatcher;
use oxphp::metrics::Metrics;
use oxphp::server::Server;

/// Start a cleartext test server and return its address plus the `Server`
/// handle (some tests need it to trigger `shutdown()` mid-connection).
///
/// The dispatcher is frozen here; pass it unfrozen. `entry_file` is a name
/// relative to `document_root` (Framework-mode routing) or `None`.
pub async fn start_test_server(
    document_root: &Path,
    h2: &H2Config,
    entry_file: Option<&str>,
    metrics: Arc<Metrics>,
    mut dispatcher: EventDispatcher,
) -> (SocketAddr, Arc<Server>) {
    let config = ServerConfig::new("127.0.0.1:0".to_string(), document_root.to_path_buf());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let executor: Arc<dyn oxphp::executor::ScriptExecutor> =
        Arc::new(oxphp::executor::stub::StubExecutor::new());

    dispatcher.freeze();

    let server = Arc::new(Server::new(
        &config,
        h2,
        executor,
        metrics,
        Arc::new(dispatcher),
        None,       // no TLS — cleartext
        0,          // compression disabled in tests
        512 * 1024, // max_query_body: 512 KB
        entry_file.map(|name| document_root.join(name)),
        false, // worker_mode_enabled
        Some("public, max-age=86400".to_string()),
        false,                            // static_revalidate
        Arc::new(AtomicBool::new(false)), // shutdown flag
    ));

    let accept_server = Arc::clone(&server);
    tokio::spawn(async move {
        loop {
            let (stream, remote_addr) = match listener.accept().await {
                Ok(conn) => conn,
                Err(_) => break,
            };
            let server = Arc::clone(&accept_server);
            tokio::spawn(async move {
                let _ = server.handle_connection(stream, remote_addr).await;
            });
        }
    });

    (addr, server)
}

/// A minimal hand-rolled HTTP/2 client: just enough framing and static-table
/// HPACK to open streams, reset them, and parse the server's control frames.
/// Used for the abusive cases the RFC-compliant `h2` client refuses to do
/// (http2_limits_tests) and for observing raw control frames like GOAWAY
/// during a drain (graceful_drain_tests).
pub mod raw {
    use std::net::SocketAddr;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    const PREFACE: &[u8] = b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n";

    // Frame types (RFC 9113 §6).
    const FRAME_HEADERS: u8 = 0x1;
    pub const FRAME_RST_STREAM: u8 = 0x3;
    pub const FRAME_SETTINGS: u8 = 0x4;
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
