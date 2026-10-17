//! HTTP/1.1 write backpressure: a client that stops reading must stop the
//! server from pulling more of a streamed body than its buffers hold.
//!
//! The body under test has the shape of a streaming PHP response — a bounded
//! channel that a thread fills with `blocking_send` — so the producer's count
//! of chunks handed over is how much of the body the HTTP side has accepted,
//! the channel included. Static files larger than the in-memory cache, sent
//! uncompressed, stream the same way, from a reader that is always ready;
//! nothing here needs PHP.

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpSocket;
use tokio::time::{sleep, timeout, Duration, Instant};

use oxphp::config::H2Config;
use oxphp::events::EventDispatcher;
use oxphp::executor::{ExecuteResult, ScriptExecutor};
use oxphp::metrics::Metrics;
use oxphp::types::{ScriptRequest, ScriptResponse};

const CHUNK: usize = 64 * 1024;
/// 128 MiB in all — far more than every buffer between the producer and the
/// client put together.
const TOTAL_CHUNKS: usize = 2048;
/// The capacity of the channel a streaming PHP response writes into.
const CHANNEL_CAPACITY: usize = 64;
/// What a client that reads nothing may cost. The channel (64 chunks), the
/// connection's write buffer (about 400 KiB) and the two loopback socket
/// buffers (a few MiB) come to about 140 chunks; the rest is room for kernels
/// that size socket buffers larger. A server that does not wait for the socket
/// takes all 2048.
const STALLED_CEILING: usize = 512;

/// Answers every request with a streaming body, produced on a thread of its
/// own the way a PHP worker produces one, and counts the chunks the HTTP side
/// has accepted.
struct StreamingExecutor {
    handed_over: Arc<AtomicUsize>,
}

impl ScriptExecutor for StreamingExecutor {
    fn execute(&self, request: ScriptRequest) -> ExecuteResult {
        request.cancel_state.mark_taken();
        let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(CHANNEL_CAPACITY);
        let handed_over = Arc::clone(&self.handed_over);
        std::thread::spawn(move || {
            // One allocation shared by every chunk: whatever the test process
            // grows by is the HTTP layer's own buffering.
            let chunk = Bytes::from(vec![b'x'; CHUNK]);
            for _ in 0..TOTAL_CHUNKS {
                if tx.blocking_send(chunk.clone()).is_err() {
                    return;
                }
                handed_over.fetch_add(1, Ordering::SeqCst);
            }
        });
        ExecuteResult::Immediate(ScriptResponse {
            stream_rx: Some(rx),
            ..ScriptResponse::default()
        })
    }

    fn shutdown(&self) {}
}

/// Waits for the producer to start, then reads its count until it has not
/// moved for 500 ms or has reached the whole body, and returns it.
async fn settled(count: &AtomicUsize) -> usize {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut last = 0;
    let mut still_since = Instant::now();
    loop {
        sleep(Duration::from_millis(50)).await;
        let now = count.load(Ordering::SeqCst);
        if now == TOTAL_CHUNKS || Instant::now() > deadline {
            return now;
        }
        if now != last {
            last = now;
            still_since = Instant::now();
        } else if now > 0 && still_since.elapsed() >= Duration::from_millis(500) {
            return now;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_client_that_stops_reading_stops_the_server_pulling_the_body() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("stream.php"), "<?php\n").unwrap();
    let handed_over = Arc::new(AtomicUsize::new(0));
    let executor = Arc::new(StreamingExecutor {
        handed_over: Arc::clone(&handed_over),
    });
    let (addr, _server) = common::start_test_server_with_executor(
        dir.path(),
        &H2Config::default(),
        None,
        Arc::new(Metrics::new()),
        EventDispatcher::new(),
        oxphp::server::compression::Levels::default(),
        executor,
    )
    .await;

    // Set before connecting, so the window the client advertises is small
    // from the first segment and the kernel does not grow it.
    let socket = TcpSocket::new_v4().unwrap();
    socket.set_recv_buffer_size(CHUNK as u32).unwrap();
    let mut sock = socket.connect(addr).await.unwrap();
    sock.write_all(b"GET /stream.php HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();

    let stalled = settled(&handed_over).await;
    // Premise: the body was being written. A count at or below the channel's
    // capacity means the HTTP side never took a chunk, and the bound below
    // would hold for a body nobody was sending.
    assert!(
        stalled > CHANNEL_CAPACITY,
        "the HTTP side never took a chunk from the body ({stalled} handed over)"
    );
    assert!(
        stalled < STALLED_CEILING,
        "a client reading nothing made the server take {stalled} of {TOTAL_CHUNKS} chunks \
         ({} MiB): it is not waiting for the socket before pulling more of the body",
        stalled * CHUNK / (1024 * 1024)
    );

    // The other half: the bound came from waiting, not from a body that died.
    // Once the client reads, all of it arrives.
    let mut raw = Vec::with_capacity(TOTAL_CHUNKS * CHUNK + 64 * 1024);
    timeout(Duration::from_secs(60), sock.read_to_end(&mut raw))
        .await
        .expect("the rest of the body did not arrive once the client read it")
        .unwrap();
    assert_eq!(
        chunked_body_len(&raw),
        TOTAL_CHUNKS * CHUNK,
        "the response did not carry the whole body"
    );
    assert_eq!(handed_over.load(Ordering::SeqCst), TOTAL_CHUNKS);
}

/// Parses a raw `200` response with a chunked body and returns the length of
/// the body it decodes. Fails unless the body ends with its last chunk, so a
/// response cut short is not counted as delivered.
fn chunked_body_len(raw: &[u8]) -> usize {
    let find = |hay: &[u8], needle: &[u8]| hay.windows(needle.len()).position(|w| w == needle);
    let head_end = find(raw, b"\r\n\r\n").expect("no end of headers") + 4;
    let head = String::from_utf8_lossy(&raw[..head_end]).to_ascii_lowercase();
    assert!(
        head.starts_with("http/1.1 200"),
        "unexpected response head: {head}"
    );
    assert!(
        head.contains("\r\ntransfer-encoding: chunked\r\n"),
        "the body is not chunked: {head}"
    );
    let mut rest = &raw[head_end..];
    let mut body = 0;
    loop {
        let line_end = find(rest, b"\r\n").expect("the body ends inside a chunk-size line");
        let size = usize::from_str_radix(std::str::from_utf8(&rest[..line_end]).unwrap(), 16)
            .expect("malformed chunk size");
        rest = &rest[line_end + 2..];
        if size == 0 {
            assert_eq!(
                rest, b"\r\n",
                "the last chunk is not followed by the end of the body"
            );
            return body;
        }
        assert_eq!(
            rest.get(size..size + 2),
            Some(&b"\r\n"[..]),
            "the body ends inside a chunk after {body} bytes"
        );
        body += size;
        rest = &rest[size + 2..];
    }
}
