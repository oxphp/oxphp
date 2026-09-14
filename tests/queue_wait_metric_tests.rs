//! What `oxphp_queue_wait_us` measures: how long a request waited before a
//! worker took it off the queue.
//!
//! The end of that wait is the pickup, and nothing after it belongs in the
//! reading — not the worker preparing the request, not the handoff of its
//! answer to the request's task, not the runtime getting round to that task.
//! Nothing in these tests needs PHP: the executor below stands in for a worker
//! and says when it took the request, which is all the dispatch side has to go
//! on.

mod common;

use std::net::SocketAddr;
use std::sync::Arc;

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::time::{sleep, Duration};

use oxphp::events::EventDispatcher;
use oxphp::executor::{ExecuteResult, Queued, ScriptExecutor};
use oxphp::metrics::Metrics;
use oxphp::types::{ScriptRequest, ScriptResponse};

/// Long enough to stand far above anything the dispatch path itself costs, so
/// a reading on either side of it is unambiguous.
const DELAY: Duration = Duration::from_millis(40);

/// When, relative to the delay, the stand-in worker takes the request.
#[derive(Clone, Copy)]
enum Pickup {
    /// At once — the delay is spent after the pickup, before the answer.
    BeforeDelay,
    /// At the end of the delay — the request waits the whole of it.
    AfterDelay,
    /// Never: the answer arrives with no pickup behind it.
    Never,
}

/// An executor that queues every request and answers it with a plain 200
/// after `DELAY`, reporting no execution time at all, so whatever the
/// histogram reads is time the dispatch side attributed to waiting.
struct DelayedExecutor {
    pickup: Pickup,
}

impl ScriptExecutor for DelayedExecutor {
    fn execute(&self, request: ScriptRequest) -> ExecuteResult {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let pickup = self.pickup;
        let cancel_state = request.cancel_state;
        if let Pickup::BeforeDelay = pickup {
            cancel_state.mark_taken();
        }
        tokio::spawn(async move {
            sleep(DELAY).await;
            if let Pickup::AfterDelay = pickup {
                cancel_state.mark_taken();
            }
            let _ = tx.send(ScriptResponse {
                execution_time_us: 0,
                ..Default::default()
            });
        });
        ExecuteResult::Deferred(Queued {
            rx,
            deadline: None,
            // No deadline, so nothing reads it.
            wait_at_ceiling: false,
        })
    }

    fn shutdown(&self) {}
}

async fn start(
    document_root: &std::path::Path,
    metrics: Arc<Metrics>,
    pickup: Pickup,
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
        Arc::new(DelayedExecutor { pickup }),
    )
    .await;
    addr
}

/// Reads one `name value` line out of the exposition text.
fn series(metrics: &Metrics, key: &str) -> u64 {
    metrics
        .to_prometheus()
        .lines()
        .find_map(|line| line.strip_prefix(key)?.trim().parse::<u64>().ok())
        .unwrap_or_else(|| panic!("no series {key} in the exposition"))
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

async fn serve_one(pickup: Pickup) -> Arc<Metrics> {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.php"), "<?php\n").unwrap();
    let metrics = Arc::new(Metrics::new());
    let addr = start(dir.path(), Arc::clone(&metrics), pickup).await;
    assert_eq!(get_status(addr).await, "HTTP/1.1 200 OK");
    metrics
}

const COUNT: &str = "oxphp_queue_wait_us_count";
const SUM: &str = "oxphp_queue_wait_us_sum";

/// A request taken at once and answered late did not wait: the time between
/// the pickup and the answer arriving is the worker's and the handoff's, and
/// reading it as queue wait tells an operator the pool is short of workers
/// when what is slow is everything around them.
#[tokio::test]
async fn time_after_the_pickup_is_not_queue_wait() {
    let metrics = serve_one(Pickup::BeforeDelay).await;
    assert_eq!(
        series(&metrics, COUNT),
        1,
        "the request was not recorded at all"
    );
    let sum = series(&metrics, SUM);
    assert!(
        sum < DELAY.as_micros() as u64 / 4,
        "a request picked up at once was recorded as waiting {sum} µs — the time spent after its pickup"
    );
}

/// The control for the test above: a request that does wait is still measured,
/// for at least as long as it waited. Without it, a histogram that recorded
/// nothing but zeros would pass.
#[tokio::test]
async fn a_wait_before_the_pickup_is_recorded() {
    let metrics = serve_one(Pickup::AfterDelay).await;
    assert_eq!(
        series(&metrics, COUNT),
        1,
        "the request was not recorded at all"
    );
    let sum = series(&metrics, SUM);
    assert!(
        sum >= DELAY.as_micros() as u64,
        "a request that waited {DELAY:?} for its pickup was recorded as waiting only {sum} µs"
    );
}

/// An answer with no pickup behind it has no pickup latency to contribute. A
/// worker in the server's own pools stamps every request it takes; an executor
/// that does not say when it took one gives the histogram nothing to measure,
/// and a reading made up from when the answer arrived is not a pickup latency.
#[tokio::test]
async fn an_answer_with_no_pickup_is_not_recorded() {
    let metrics = serve_one(Pickup::Never).await;
    assert_eq!(
        series(&metrics, COUNT),
        0,
        "an answer no worker took the request for was recorded as queue wait"
    );
}
