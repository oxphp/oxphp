//! What `oxphp_request_cancelled_total` reports when a client walks away.
//!
//! The counter exists so an operator can separate a client giving up from a
//! server failing, and the commonest shape of the first is a client that
//! leaves while its request is still waiting for a worker. Nothing in these
//! tests needs PHP: the executor below holds the request open, which is the
//! only thing the stub executor cannot do, and the dispatch side that does the
//! counting is the same one a real pool sits behind.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::Mutex;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::{sleep, Duration, Instant};

use oxphp::events::EventDispatcher;
use oxphp::executor::{ExecuteResult, Queued, ScriptExecutor};
use oxphp::metrics::Metrics;
use oxphp::types::{ScriptRequest, ScriptResponse};

/// An executor that accepts requests and never answers them — a pool whose
/// every worker is busy. The senders are parked rather than dropped: dropping
/// one answers the dispatch side with a worker error, which is a different
/// outcome from the one under test.
struct HoldingExecutor {
    held: Mutex<Vec<tokio::sync::oneshot::Sender<ScriptResponse>>>,
}

impl HoldingExecutor {
    fn new() -> Self {
        Self {
            held: Mutex::new(Vec::new()),
        }
    }
}

impl ScriptExecutor for HoldingExecutor {
    fn execute(&self, _request: ScriptRequest) -> ExecuteResult {
        let (tx, rx) = tokio::sync::oneshot::channel();
        self.held.lock().unwrap().push(tx);
        // No deadline: fail-fast mode's shape, so nothing on the waiting side
        // can answer this request and the only way it ends is the client.
        ExecuteResult::Deferred(Queued {
            rx,
            deadline: None,
            // No deadline, so nothing reads it: the flag says whether a wait
            // that ran out ran out over the configured budget, and this one
            // has no budget to run out of.
            wait_at_ceiling: false,
        })
    }

    fn shutdown(&self) {}
}

/// An executor that answers every queued request with a fixed response after
/// a measurable wait, taking the request only at the end of it, as a worker
/// that reached the request only then would. The mark is what gets a request
/// answered this way recorded at all; the wait only keeps its reading off
/// zero.
struct AnsweringExecutor {
    answer: fn() -> ScriptResponse,
}

impl ScriptExecutor for AnsweringExecutor {
    fn execute(&self, request: ScriptRequest) -> ExecuteResult {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let answer = self.answer;
        let cancel_state = request.cancel_state;
        tokio::spawn(async move {
            sleep(Duration::from_millis(20)).await;
            cancel_state.mark_taken();
            let _ = tx.send(answer());
        });
        ExecuteResult::Deferred(Queued {
            rx,
            deadline: None,
            // No deadline, so nothing reads it: the flag says whether a wait
            // that ran out ran out over the configured budget, and this one
            // has no budget to run out of.
            wait_at_ceiling: false,
        })
    }

    fn shutdown(&self) {}
}

async fn start(document_root: &std::path::Path, metrics: Arc<Metrics>) -> SocketAddr {
    start_with(document_root, metrics, Arc::new(HoldingExecutor::new())).await
}

async fn start_with(
    document_root: &std::path::Path,
    metrics: Arc<Metrics>,
    executor: Arc<dyn ScriptExecutor>,
) -> SocketAddr {
    let mut dispatcher = EventDispatcher::new();
    dispatcher.on(oxphp::handlers::request_id::RequestIdGenerator);
    dispatcher.on(oxphp::handlers::metrics::MetricsRequestHandler::new(
        Arc::clone(&metrics),
    ));
    dispatcher.on(oxphp::handlers::metrics::MetricsResponseHandler::new(
        Arc::clone(&metrics),
    ));

    let (addr, _server) = common::start_test_server_with_executor(
        document_root,
        &oxphp::config::H2Config::default(),
        None,
        metrics,
        dispatcher,
        oxphp::server::compression::Levels::default(),
        executor,
    )
    .await;
    addr
}

/// Reads one `name{labels} value` line out of the exposition text.
fn series(metrics: &Metrics, key: &str) -> u64 {
    metrics
        .to_prometheus()
        .lines()
        .find_map(|line| line.strip_prefix(key)?.trim().parse::<u64>().ok())
        .unwrap_or_else(|| panic!("no series {key} in the exposition"))
}

/// Polls until `key` reaches `want`, or gives up. Returns what it last read,
/// so the caller reports the value rather than a timeout.
async fn wait_for(metrics: &Metrics, key: &str, want: u64) -> u64 {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let got = series(metrics, key);
        if got >= want || Instant::now() > deadline {
            return got;
        }
        sleep(Duration::from_millis(10)).await;
    }
}

const CANCELLED_CLIENT_ABORT: &str = "oxphp_request_cancelled_total{reason=\"client_abort\"}";

#[tokio::test]
async fn a_client_that_leaves_mid_request_is_counted_as_a_client_abort() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.php"), "<?php\n").unwrap();
    let metrics = Arc::new(Metrics::new());
    let addr = start(dir.path(), Arc::clone(&metrics)).await;

    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"GET /index.php HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    sock.flush().await.unwrap();

    // Negative control. Without it the assertion below passes on a run where
    // the request never reached the executor at all — a 404, a route that
    // resolved to a static file — and the count would be zero for a reason
    // that has nothing to do with cancellation.
    assert_eq!(
        wait_for(&metrics, "oxphp_pending_requests", 1).await,
        1,
        "the request never reached the pool, so nothing here is about a client abort"
    );
    assert_eq!(
        series(&metrics, CANCELLED_CLIENT_ABORT),
        0,
        "counted before the client had gone anywhere"
    );

    drop(sock);

    assert_eq!(
        wait_for(&metrics, CANCELLED_CLIENT_ABORT, 1).await,
        1,
        "a client that closed the connection on an unanswered request was not counted"
    );
    // The request is gone from the pool's books too: the counter must not be
    // reporting a cancellation for work the server still believes is running.
    assert_eq!(series(&metrics, "oxphp_pending_requests"), 0);
}

/// The counter is per request, not per cancellation signal. Two clients
/// leaving must read as two, and one must never read as more than one —
/// a request cancelled on a path that both fires the abort guard and brings a
/// worker's answer back would otherwise be counted twice.
#[tokio::test]
async fn each_departed_client_is_counted_exactly_once() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.php"), "<?php\n").unwrap();
    let metrics = Arc::new(Metrics::new());
    let addr = start(dir.path(), Arc::clone(&metrics)).await;

    // Held open in a vector rather than dropped inside the loop: a socket
    // closed before the server has read its request line leaves nothing to
    // cancel, and the control below could then never reach three.
    let mut socks = Vec::new();
    for _ in 0..3 {
        let mut sock = TcpStream::connect(addr).await.unwrap();
        sock.write_all(b"GET /index.php HTTP/1.1\r\nHost: localhost\r\n\r\n")
            .await
            .unwrap();
        sock.flush().await.unwrap();
        socks.push(sock);
    }

    assert_eq!(
        wait_for(&metrics, "oxphp_pending_requests", 3).await,
        3,
        "not all three requests reached the pool"
    );

    drop(socks);

    assert_eq!(
        wait_for(&metrics, CANCELLED_CLIENT_ABORT, 3).await,
        3,
        "three departed clients were not counted as three cancellations"
    );
    // And no more arrive afterwards. A second increment for the same request
    // would land when its answer comes back — held here forever, so this
    // window is what the pool's own answer would have to beat.
    sleep(Duration::from_millis(200)).await;
    assert_eq!(
        series(&metrics, CANCELLED_CLIENT_ABORT),
        3,
        "one request was counted more than once"
    );
}

/// Reads one request to completion and returns its status line.
async fn get_status(addr: SocketAddr) -> String {
    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"GET /index.php HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    sock.flush().await.unwrap();
    let mut buf = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut sock, &mut buf)
        .await
        .unwrap();
    String::from_utf8_lossy(&buf)
        .lines()
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A 499 owed to a departed client means two different things depending on
/// whether a worker ever reached the request, and `oxphp_queue_wait_us` is
/// where the difference has to show. The histogram measures how long a
/// request waited before a worker picked it up; a request whose budget ran
/// out with nobody picking it up has no such measurement to contribute, and
/// admitting one lets a whole `QUEUE_WAIT_TIMEOUT_MS` in as though it were
/// pickup latency — the very distortion the 529 on that path is kept out for.
#[tokio::test]
async fn a_499_for_a_request_no_worker_reached_stays_out_of_the_queue_wait_histogram() {
    const COUNT: &str = "oxphp_queue_wait_us_count";
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.php"), "<?php\n").unwrap();

    // The control first, so a zero below cannot be the histogram never
    // moving on this path at all: the same 499, for a client that left after
    // a worker had already taken its request, is an ordinary pickup latency.
    let picked_up = Arc::new(Metrics::new());
    let addr = start_with(
        dir.path(),
        Arc::clone(&picked_up),
        Arc::new(AnsweringExecutor {
            answer: ScriptResponse::client_closed,
        }),
    )
    .await;
    assert_eq!(get_status(addr).await, "HTTP/1.1 499 <none>");
    assert_eq!(
        series(&picked_up, COUNT),
        1,
        "a request a worker did take was left out of the pickup-latency histogram"
    );

    let unpicked = Arc::new(Metrics::new());
    let addr = start_with(
        dir.path(),
        Arc::clone(&unpicked),
        Arc::new(AnsweringExecutor {
            answer: ScriptResponse::client_closed_unpicked,
        }),
    )
    .await;
    assert_eq!(
        get_status(addr).await,
        "HTTP/1.1 499 <none>",
        "the two answers must differ only in what they are counted as"
    );
    assert_eq!(
        series(&unpicked, COUNT),
        0,
        "a wait that ended with no worker picking the request up was recorded as pickup latency"
    );
}
