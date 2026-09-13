//! How a finished connection's error is classified for the log.
//!
//! A client that stops waiting is the most ordinary event a public server
//! sees, and the line it writes decides whether an operator reads it as their
//! own fault. The error that arrives is hyper's, so the classification is
//! pinned against a real one produced the way production produces it — a
//! complete request, no response yet, and the client closing the socket —
//! rather than against a string that happened to look right.

use std::convert::Infallible;
use std::time::Duration;

use bytes::Bytes;
use http::{Request, Response};
use http_body_util::Full;
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

/// Serves one connection with a handler that never answers, and returns what
/// `serve_connection` ended with once `abort` has run against it.
///
/// The handler has to outlive the client for the reproduction to be the
/// production one: hyper reports an idle close cleanly, and only a close with
/// a message still owed reaches the error this file is about.
async fn serve_one_and_abort<F, Fut>(abort: F) -> hyper::Result<()>
where
    F: FnOnce(TcpStream) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = tokio::sync::oneshot::channel();

    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let service = service_fn(|_req: Request<Incoming>| async move {
            // Longer than the whole test: this request is never answered, so
            // the connection still owes a response when the client goes.
            tokio::time::sleep(Duration::from_secs(30)).await;
            Ok::<_, Infallible>(Response::new(Full::new(Bytes::new())))
        });
        let result = hyper::server::conn::http1::Builder::new()
            // Armed so the second case below has a deadline of ours to
            // overrun. It disarms once the header block is complete, so the
            // case that finishes its request is unaffected by it.
            .timer(hyper_util::rt::TokioTimer::new())
            .header_read_timeout(Duration::from_millis(200))
            .serve_connection(TokioIo::new(stream), service)
            .await;
        let _ = tx.send(result);
    });

    let sock = TcpStream::connect(addr).await.unwrap();
    abort(sock).await;
    rx.await.unwrap()
}

/// Sends a complete request, lets the server pick it up, then closes.
async fn request_then_close(mut sock: TcpStream) {
    sock.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n")
        .await
        .unwrap();
    sock.flush().await.unwrap();
    // Long enough for hyper to have read the request and handed it to the
    // handler. Closing before that leaves an idle connection, which hyper
    // reports as a clean end and which is not the case under test.
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(sock);
}

/// The upstream premise the classification rests on, re-derived here rather
/// than taken from hyper's documentation: a client that leaves an unanswered
/// request behind ends the connection with `is_incomplete_message()`.
#[tokio::test]
async fn a_departing_client_ends_the_connection_with_an_incomplete_message() {
    let err = serve_one_and_abort(request_then_close)
        .await
        .expect_err("a client that left mid-request ended the connection cleanly");

    assert!(
        err.is_incomplete_message(),
        "not the error this classification is built on: {err}"
    );
    assert!(
        !err.is_timeout(),
        "a client going away is not a timeout of ours: {err}"
    );
    // Why the text cannot decide it. The only thing separating a client
    // leaving from a fault of the server's used to be whether the message
    // carried the word "timeout", and this one does not.
    assert!(
        !err.to_string().contains("timeout"),
        "the substring rule would have to have caught this: {err}"
    );
}

/// And the classifier reads that error the way the log needs it read — after
/// the trip through the box that `hyper_util`'s connection future puts it in,
/// which is the only form the caller ever sees.
#[tokio::test]
async fn a_departing_client_is_classified_as_gone_not_as_a_fault() {
    let err = serve_one_and_abort(request_then_close).await.unwrap_err();
    let boxed: oxphp::types::BoxError = Box::new(err);

    assert_eq!(
        oxphp::server::classify_connection_end(boxed.as_ref()),
        oxphp::server::ConnectionEnd::ClientGone
    );
}

/// A header the client never finished sending is hyper's own timeout, and it
/// stays a warning: the connection was held open against a deadline of ours.
#[tokio::test]
async fn a_client_that_never_finishes_its_headers_is_a_timeout() {
    let err = serve_one_and_abort(|mut sock| async move {
        // A request line and no terminating blank line, held open past the
        // header-read deadline set below.
        sock.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n")
            .await
            .unwrap();
        sock.flush().await.unwrap();
        tokio::time::sleep(Duration::from_millis(400)).await;
        drop(sock);
    })
    .await
    .expect_err("an unfinished header block ended the connection cleanly");

    assert!(err.is_timeout(), "not a timeout: {err}");
    let boxed: oxphp::types::BoxError = Box::new(err);
    assert_eq!(
        oxphp::server::classify_connection_end(boxed.as_ref()),
        oxphp::server::ConnectionEnd::Timeout
    );
}

/// Nothing else is quietened. An error the classifier does not recognise is
/// the server's until someone has looked at it.
#[test]
fn an_unrecognised_error_stays_a_fault() {
    let boxed: oxphp::types::BoxError = "something new".into();
    assert_eq!(
        oxphp::server::classify_connection_end(boxed.as_ref()),
        oxphp::server::ConnectionEnd::Fault
    );
}

/// The I/O half, and the chain walk that reaches it. The error the caller
/// holds is boxed and may carry its real cause a link down — that is why the
/// classifier walks `source()` rather than reading only the head — and a
/// connection the peer reset is that peer leaving however it is wrapped.
#[test]
fn an_io_cause_one_link_down_is_classified_on_its_kind() {
    #[derive(Debug)]
    struct Wrapped(std::io::Error);

    impl std::fmt::Display for Wrapped {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "error shutting down connection")
        }
    }

    impl std::error::Error for Wrapped {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(&self.0)
        }
    }

    let classify = |kind: std::io::ErrorKind| {
        let boxed: oxphp::types::BoxError = Box::new(Wrapped(std::io::Error::new(kind, "peer")));
        oxphp::server::classify_connection_end(boxed.as_ref())
    };

    use std::io::ErrorKind::*;
    for kind in [
        ConnectionReset,
        ConnectionAborted,
        BrokenPipe,
        NotConnected,
        UnexpectedEof,
    ] {
        assert_eq!(
            classify(kind),
            oxphp::server::ConnectionEnd::ClientGone,
            "{kind:?} is a peer that went away"
        );
    }
    assert_eq!(
        classify(TimedOut),
        oxphp::server::ConnectionEnd::Timeout,
        "a socket-level timeout is a deadline, not a departure"
    );
    // The wrapper's own text says nothing, so an unrecognised kind must not
    // be quietened by the walk having found *an* I/O error.
    assert_eq!(
        classify(PermissionDenied),
        oxphp::server::ConnectionEnd::Fault,
        "an I/O failure that is not a departure stays the server's"
    );
}

/// The walk is bounded, and this is where the bound lands. A `source()`
/// chain that cycles would otherwise spin the connection's task at full
/// tilt, holding one of `MAX_CONNECTIONS` and never writing the line the
/// classification was computed for — so the walk gives up rather than
/// follows a chain to wherever it goes. A cyclic chain cannot be asserted
/// on without hanging the unbounded version, so the bound is pinned from the
/// other side: a cause deep enough to be past it is not read.
#[test]
fn a_cause_past_the_walks_bound_is_not_read() {
    #[derive(Debug)]
    struct Link(Box<dyn std::error::Error + Send + Sync + 'static>);

    impl std::fmt::Display for Link {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "link")
        }
    }

    impl std::error::Error for Link {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            Some(self.0.as_ref())
        }
    }

    // `depth` links of nothing in particular, over a cause the classifier
    // does recognise.
    let chain = |depth: usize| {
        let mut e: Box<dyn std::error::Error + Send + Sync + 'static> = Box::new(
            std::io::Error::new(std::io::ErrorKind::ConnectionReset, "peer"),
        );
        for _ in 0..depth {
            e = Box::new(Link(e));
        }
        let boxed: oxphp::types::BoxError = e;
        oxphp::server::classify_connection_end(boxed.as_ref())
    };

    // Inside the bound the same cause is still found, so the assertion below
    // is about the depth and not about the chain being unreadable.
    assert_eq!(chain(15), oxphp::server::ConnectionEnd::ClientGone);
    assert_eq!(
        chain(16),
        oxphp::server::ConnectionEnd::Fault,
        "the walk followed a chain past the point where it gives up"
    );
}
