//! Why a request was refused reaches the handlers that see it finish.
//!
//! Every overload refusal answers with the same `529`, whether it was decided
//! on the dispatch side or on a worker thread — so the status says nothing
//! about which limit was hit, and a `529` the application returned itself
//! looks the same. The reason has to travel with the answer from wherever it
//! was decided to `RequestComplete`, which is where the trace span and
//! anything else that describes the request is built. Nothing here needs PHP:
//! the executor below stands in for the pool, refuses each way the pool
//! refuses, and once answers the way an application can.

mod common;

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;

use oxphp::events::{EventDispatcher, EventHandler, Propagation, RequestComplete};
use oxphp::executor::admission::ShedReason;
use oxphp::executor::{ExecuteResult, Queued, ScriptExecutor};
use oxphp::metrics::Metrics;
use oxphp::types::{ScriptRequest, ScriptResponse};

/// How the stand-in pool answers.
#[derive(Clone, Copy)]
enum Answer {
    /// Refused on arrival, without reaching the queue.
    Rejected(ShedReason),
    /// Refused after waiting for a place in the queue.
    RefusedWhileAdmitting(ShedReason),
    /// Refused through the response channel, the way a worker refuses a
    /// request it reached past its deadline.
    RefusedByWorker(ShedReason),
    /// Not refused at all: the script answered `529` itself.
    Application529,
}

struct StandIn {
    answer: Answer,
}

impl ScriptExecutor for StandIn {
    fn execute(&self, request: ScriptRequest) -> ExecuteResult {
        match self.answer {
            Answer::Rejected(reason) => ExecuteResult::Rejected(ScriptResponse::overloaded(reason)),
            Answer::RefusedWhileAdmitting(reason) => {
                ExecuteResult::Admitting(Box::pin(async move {
                    Err(ScriptResponse::overloaded(reason))
                }))
            }
            Answer::RefusedByWorker(reason) => {
                let (tx, rx) = tokio::sync::oneshot::channel();
                request.cancel_state.mark_taken();
                let _ = tx.send(ScriptResponse::overloaded(reason));
                ExecuteResult::Deferred(Queued {
                    rx,
                    deadline: None,
                    // No deadline, so nothing reads it.
                    wait_at_ceiling: false,
                })
            }
            Answer::Application529 => ExecuteResult::Immediate(ScriptResponse {
                status: 529,
                ..Default::default()
            }),
        }
    }

    fn shutdown(&self) {}
}

/// A finished request's status, and the reason it was refused for, if any.
type Outcome = (u16, Option<ShedReason>);

/// Records what `RequestComplete` said about the refusal.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Outcome>>>);

impl EventHandler<RequestComplete> for Seen {
    fn handle(&self, event: &mut RequestComplete) -> Propagation {
        self.0
            .lock()
            .unwrap()
            .push((event.status, event.shed_reason));
        Propagation::Continue
    }
}

async fn start(document_root: &std::path::Path, answer: Answer, seen: Seen) -> SocketAddr {
    let metrics = Arc::new(Metrics::new());
    let mut dispatcher = EventDispatcher::new();
    dispatcher.on(oxphp::handlers::request_id::RequestIdGenerator);
    dispatcher.on(seen);

    let (addr, _server) = common::start_test_server_with_executor(
        document_root,
        &oxphp::config::H2Config::default(),
        None,
        metrics,
        dispatcher,
        oxphp::server::compression::Levels::default(),
        Arc::new(StandIn { answer }),
    )
    .await;
    addr
}

/// Serves one request answered `answer` and returns what `RequestComplete`
/// carried for it.
async fn complete(answer: Answer) -> Outcome {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.php"), "<?php\n").unwrap();
    let seen = Seen::default();
    let addr = start(dir.path(), answer, seen.clone()).await;

    let mut sock = TcpStream::connect(addr).await.unwrap();
    sock.write_all(b"GET /index.php HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
        .await
        .unwrap();
    sock.flush().await.unwrap();
    let mut buf = Vec::new();
    tokio::io::AsyncReadExt::read_to_end(&mut sock, &mut buf)
        .await
        .unwrap();

    let seen = seen.0.lock().unwrap();
    assert_eq!(seen.len(), 1, "one RequestComplete per request");
    seen[0]
}

#[tokio::test]
async fn a_refusal_on_arrival_carries_its_reason() {
    assert_eq!(
        complete(Answer::Rejected(ShedReason::QueueFull)).await,
        (529, Some(ShedReason::QueueFull))
    );
}

#[tokio::test]
async fn a_refusal_while_waiting_for_a_slot_carries_its_reason() {
    assert_eq!(
        complete(Answer::RefusedWhileAdmitting(ShedReason::WaitingBytes)).await,
        (529, Some(ShedReason::WaitingBytes))
    );
}

#[tokio::test]
async fn a_refusal_from_a_worker_carries_its_reason() {
    assert_eq!(
        complete(Answer::RefusedByWorker(ShedReason::WaitTimeout)).await,
        (529, Some(ShedReason::WaitTimeout))
    );
}

/// The control: a `529` nobody refused carries no reason, so a reason on the
/// others is the refusal's and not the status code's.
#[tokio::test]
async fn an_application_529_carries_no_reason() {
    assert_eq!(complete(Answer::Application529).await, (529, None));
}
