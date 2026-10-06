//! The span exporter the OTel plugin hands to the batch span processor.
//!
//! The SDK's batch processor gives the exporter each batch by value and, when
//! the export returns an error, logs it and drops the batch: it neither retries
//! nor counts what it lost, and the OTLP exporters of this SDK generation do
//! not retry either. This wrapper sits in front of the OTLP exporter — the last
//! place that still holds the batch — to count the exports that fail and to
//! retry, once, an export whose first attempt failed fast.
//!
//! The gRPC channel that exporter sends over is built here too; see
//! [`grpc_channel`].

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use hyper_util::client::legacy::connect::HttpConnector;
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::trace::{SpanData, SpanExporter};
use opentelemetry_sdk::Resource;
use tonic::transport::{Channel, ClientTlsConfig};

/// Upper bound on how long a failed first attempt may have taken and still be
/// retried; see [`retry_within`].
const RETRY_WITHIN_MAX: Duration = Duration::from_secs(1);

/// Pause before the retry. The error that ends the first attempt comes from
/// the connection's own task; the task that hands requests to that connection
/// can see the connection close a moment later, and a retry it accepts in
/// between fails on the same dead connection.
const RETRY_PAUSE: Duration = Duration::from_millis(100);

/// The window a failed first attempt must fall inside to be retried: half the
/// export timeout, and never more than a second.
///
/// A connection that fails this fast is not the collector being slow: it is a
/// reset on a connection the collector side has already forgotten, a request
/// cancelled because its connection closed, a refused connect. An attempt that
/// ran into the export timeout is never inside the window, so a slow collector
/// is not asked twice for the same batch.
pub(super) fn retry_within(timeout: Duration) -> Duration {
    (timeout / 2).min(RETRY_WITHIN_MAX)
}

/// Whether a failed export ended without an answer from the collector: its
/// connection broke, closed, or could not be made.
///
/// Both exporters hand the error back only as text. Over gRPC it is the text
/// of tonic's `Status`. tonic gives a status a source only when the request
/// failed below gRPC, and prints the transport error after `source:`. A status
/// the collector answered with — in its trailers, or inferred from the HTTP
/// status of a proxy in between — has none. Over HTTP it is reqwest's error,
/// printed with `{:?}`. A request that got no response has the error of the
/// hyper-util client underneath after `source:`. An error status the collector,
/// or a load balancer in front of it, answered with is `kind: Status(..)` with
/// no source, and an attempt that ran out of time has `TimedOut` there instead.
/// A forward proxy (`HTTPS_PROXY`) that refuses to open a tunnel to the
/// collector fails the connect, and is retried like any other connection that
/// could not be made.
///
/// The OTLP specification forbids retrying most of those answers, and the
/// delay a throttling collector asks for travels where the text leaves it out:
/// in the gRPC status details, or in the `Retry-After` header. Should an
/// upgrade of tonic, reqwest or hyper-util change the text, retries stop
/// rather than spread to those answers.
fn collector_did_not_answer(error: &str) -> bool {
    error.contains("source: tonic::transport::Error(")
        || error.contains("source: hyper_util::client::legacy::Error(")
}

/// TCP keepalive on the export connection: the first probe goes out after
/// this long without traffic, the next ones every [`TCP_KEEPALIVE_INTERVAL`].
/// The probes keep the connection in the tables of the NAT gateways and load
/// balancers in between, which may drop an idle one after a few minutes (350
/// seconds on an AWS NAT gateway, four minutes by default on an Azure load
/// balancer) — between exports, the connection is idle. They also find out a
/// peer that is gone: the connection is closed once [`TCP_KEEPALIVE_PROBES`]
/// of them go unanswered, about a minute after the peer last answered. On
/// Linux `TCP_USER_TIMEOUT` takes over that decision: the connection is closed
/// once a probe has gone out and the export timeout has passed since the peer
/// last answered, about 40 seconds in at the default timeout.
const TCP_KEEPALIVE_IDLE: Duration = Duration::from_secs(30);
const TCP_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(10);
const TCP_KEEPALIVE_PROBES: u32 = 3;

/// The channel the OTLP gRPC exporter sends over.
///
/// The channel the exporter builds itself bounds only the request: the export
/// timeout starts once a connection is ready. A connect that hangs holds the
/// export much longer than the timeout — a TLS handshake with a peer that
/// accepted the connection and never answers, for good — and a connection
/// whose far side is gone without a word fails every export sent over it, each
/// after the full timeout, until the operating system gives up on it. This
/// one bounds both by the same timeout:
///
/// - connecting — name lookup, TCP and the TLS handshake together — fails once
///   the export timeout has passed;
/// - on Linux, the connection is closed once what was sent on it has gone
///   unacknowledged for the export timeout, or could not be sent for that long
///   because the peer stopped taking anything (`TCP_USER_TIMEOUT`). The export
///   over a connection that died still fails at the export timeout, and the
///   connection is closed shortly after, so later exports connect again
///   instead of waiting out the same dead connection. One that starts before
///   then — the batch processor sends the next batch at once when a full one
///   is waiting — fails as the connection closes, usually soon enough to be
///   retried;
/// - the socket carries TCP keepalive; see [`TCP_KEEPALIVE_IDLE`].
///
/// It does not ping the collector (HTTP/2 PING), though pings would find a
/// dead connection on any system. grpc-go — the gRPC server of the OpenTelemetry
/// Collector — counts the pings of a connection across its calls, and by
/// default closes the connection with GOAWAY `too_many_pings` on the third
/// that comes, with nothing written by the server in between, sooner than five
/// minutes after the one before (two hours with no call open), failing the
/// export in flight. Pings frequent enough to find a dead connection within one
/// export get there during the export after one left unanswered past the
/// timeout.
pub(super) fn grpc_channel(
    endpoint: String,
    timeout: Duration,
    tls: Option<ClientTlsConfig>,
) -> Result<Channel, Box<dyn std::error::Error>> {
    // Parsed as a URI, as the exporter parses its own endpoint: a `unix:`
    // socket path is refused here, not sent to a host named after tonic's
    // placeholder authority by the TCP connector below.
    let mut endpoint = Channel::from_shared(endpoint)?
        .timeout(timeout)
        .connect_timeout(timeout);
    if let Some(tls) = tls {
        endpoint = endpoint.tls_config(tls)?;
    }
    // tonic bounds the whole connect made through a connector of its caller by
    // the connect timeout, TLS handshake included.
    Ok(endpoint.connect_with_connector_lazy(grpc_connector(timeout)))
}

/// The TCP connector tonic would build for this channel, plus TCP keepalive
/// and, on Linux, `TCP_USER_TIMEOUT`. Its own connect timeout is split between
/// the addresses a name resolves to, so one address that never answers does
/// not use up the whole of it.
fn grpc_connector(timeout: Duration) -> HttpConnector {
    let mut http = HttpConnector::new();
    // https is handled above this connector, by tonic's TLS layer.
    http.enforce_http(false);
    http.set_nodelay(true);
    http.set_connect_timeout(Some(timeout));
    http.set_keepalive(Some(TCP_KEEPALIVE_IDLE));
    http.set_keepalive_interval(Some(TCP_KEEPALIVE_INTERVAL));
    http.set_keepalive_retries(Some(TCP_KEEPALIVE_PROBES));
    #[cfg(target_os = "linux")]
    http.set_tcp_user_timeout(Some(timeout));
    http
}

/// Counters for `/metrics`, written by the batch processor's export thread and
/// read by the metrics endpoint.
#[derive(Debug, Default)]
pub(super) struct ExportStats {
    failures: AtomicU64,
    failed_spans: AtomicU64,
    retries: AtomicU64,
}

impl ExportStats {
    pub(super) fn collect(&self, out: &mut String) {
        let counters = [
            (
                "oxphp_otel_export_failures_total",
                "OTLP span exports that failed, after the retry where one was made. Their spans are not sent again.",
                &self.failures,
            ),
            (
                "oxphp_otel_export_failed_spans_total",
                "Spans carried by the OTLP exports that failed.",
                &self.failed_spans,
            ),
            (
                "oxphp_otel_export_retries_total",
                "OTLP span exports retried after a first attempt that failed fast, without an answer from the collector.",
                &self.retries,
            ),
        ];
        for (name, help, value) in counters {
            let _ = writeln!(out, "# HELP {name} {help}");
            let _ = writeln!(out, "# TYPE {name} counter");
            let _ = writeln!(out, "{name} {}", value.load(Ordering::Relaxed));
        }
    }
}

/// Wraps the OTLP span exporter; see the module documentation.
#[derive(Debug)]
pub(super) struct RetryingExporter<E> {
    inner: E,
    stats: Arc<ExportStats>,
    retry_within: Duration,
}

impl<E> RetryingExporter<E> {
    pub(super) fn new(inner: E, stats: Arc<ExportStats>, retry_within: Duration) -> Self {
        Self {
            inner,
            stats,
            retry_within,
        }
    }
}

impl<E: SpanExporter> SpanExporter for RetryingExporter<E> {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let spans = batch.len();
        // The inner exporter consumes the batch, so the retry needs its own copy.
        let copy = batch.clone();
        let started = Instant::now();
        let mut retried = false;
        let mut result = self.inner.export(batch).await;

        // The OTLP exporters report every failed request as `InternalFailure`:
        // a broken connection, the export timeout, or an error status the
        // collector answered with. Only a connection that failed fast is
        // retried. `AlreadyShutdown` and `Timeout` are not worth a second
        // attempt.
        if let Err(error @ OTelSdkError::InternalFailure(message)) = &result {
            if started.elapsed() < self.retry_within && collector_did_not_answer(message) {
                tracing::warn!(
                    plugin = "otel",
                    spans,
                    error = %error,
                    "OTLP span export failed, retrying once"
                );
                self.stats.retries.fetch_add(1, Ordering::Relaxed);
                // Blocking is fine here: the batch processor runs every export
                // on its own thread, not on a runtime worker.
                std::thread::sleep(RETRY_PAUSE);
                retried = true;
                result = self.inner.export(copy).await;
            }
        }

        if let Err(error) = &result {
            self.stats.failures.fetch_add(1, Ordering::Relaxed);
            self.stats
                .failed_spans
                .fetch_add(spans as u64, Ordering::Relaxed);
            tracing::error!(
                plugin = "otel",
                spans,
                retried,
                error = %error,
                "OTLP span export failed, the batch is dropped"
            );
        }
        result
    }

    fn shutdown_with_timeout(&mut self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn shutdown(&mut self) -> OTelSdkResult {
        self.inner.shutdown()
    }

    fn force_flush(&mut self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(resource);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::convert::Infallible;
    use std::net::SocketAddr;
    use std::sync::atomic::AtomicUsize;
    use std::sync::Mutex;

    use bytes::Bytes;
    use http_body_util::{BodyExt, Empty, StreamBody};
    use hyper::body::{Frame, Incoming};
    use hyper::service::service_fn;
    use hyper_util::rt::{TokioExecutor, TokioIo};
    use opentelemetry::trace::{
        Span as _, SpanContext, SpanId, SpanKind, Status, TraceFlags, TraceId, TraceState,
        Tracer as _, TracerProvider as _,
    };
    use opentelemetry::{InstrumentationScope, KeyValue};
    use opentelemetry_sdk::error::OTelSdkError;
    use opentelemetry_sdk::trace::{SpanEvents, SpanLinks};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};

    use super::super::OtelPlugin;
    use crate::config::test_env::with_env;
    use crate::plugin::PluginError;

    // ─── Unit: the wrapper around a scripted exporter ─────────────────────

    /// What one export attempt of [`Scripted`] does: how long it takes, and
    /// how it ends.
    #[derive(Debug, Clone, Copy)]
    enum Outcome {
        Ok,
        InternalFailure,
        Timeout,
        AlreadyShutdown,
    }

    /// An exporter that plays back a script of outcomes and records what it
    /// was handed.
    #[derive(Debug, Default)]
    struct Scripted {
        script: Mutex<VecDeque<(Duration, Outcome)>>,
        exported: Mutex<Vec<Vec<String>>>,
        resource: Mutex<Option<Resource>>,
        flushed: AtomicUsize,
        shut_down: Mutex<Vec<Duration>>,
        /// Calls to `shutdown()` itself, the method the batch processor
        /// calls, as opposed to `shutdown_with_timeout`.
        plain_shutdowns: AtomicUsize,
    }

    impl Scripted {
        fn new(script: &[(Duration, Outcome)]) -> Self {
            Self {
                script: Mutex::new(script.iter().copied().collect()),
                ..Self::default()
            }
        }
    }

    /// The wrapper owns its inner exporter, so the tests hand it this shared
    /// handle and keep another to read the recording back.
    #[derive(Debug)]
    struct Shared(Arc<Scripted>);

    impl SpanExporter for Shared {
        async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
            self.0
                .exported
                .lock()
                .unwrap()
                .push(batch.iter().map(|s| s.name.to_string()).collect());
            let (took, outcome) = self
                .0
                .script
                .lock()
                .unwrap()
                .pop_front()
                .expect("an export the script did not plan for");
            std::thread::sleep(took);
            match outcome {
                Outcome::Ok => Ok(()),
                Outcome::InternalFailure => Err(OTelSdkError::InternalFailure(
                    "code: 'Unknown error', message: \"transport error\", source: \
                     tonic::transport::Error(Transport, hyper::Error(Io, Kind(ConnectionReset)))"
                        .into(),
                )),
                Outcome::Timeout => Err(OTelSdkError::Timeout(took)),
                Outcome::AlreadyShutdown => Err(OTelSdkError::AlreadyShutdown),
            }
        }

        fn shutdown_with_timeout(&mut self, timeout: Duration) -> OTelSdkResult {
            self.0.shut_down.lock().unwrap().push(timeout);
            Ok(())
        }

        fn shutdown(&mut self) -> OTelSdkResult {
            self.0.plain_shutdowns.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn force_flush(&mut self) -> OTelSdkResult {
            self.0.flushed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn set_resource(&mut self, resource: &Resource) {
            *self.0.resource.lock().unwrap() = Some(resource.clone());
        }
    }

    fn span(name: &'static str) -> SpanData {
        SpanData {
            span_context: SpanContext::new(
                TraceId::from(1),
                SpanId::from(1),
                TraceFlags::SAMPLED,
                false,
                TraceState::default(),
            ),
            parent_span_id: SpanId::INVALID,
            parent_span_is_remote: false,
            span_kind: SpanKind::Server,
            name: name.into(),
            start_time: std::time::UNIX_EPOCH,
            end_time: std::time::UNIX_EPOCH,
            attributes: Vec::new(),
            dropped_attributes_count: 0,
            events: SpanEvents::default(),
            links: SpanLinks::default(),
            status: Status::Unset,
            instrumentation_scope: InstrumentationScope::default(),
        }
    }

    /// Export `batch` through a wrapper around `script` with a retry window of
    /// `within`, and return the result, the recording and the counters.
    fn export_through(
        script: &[(Duration, Outcome)],
        within: Duration,
        batch: Vec<SpanData>,
    ) -> (OTelSdkResult, Arc<Scripted>, Arc<ExportStats>) {
        let inner = Arc::new(Scripted::new(script));
        let stats = Arc::new(ExportStats::default());
        let exporter = RetryingExporter::new(Shared(inner.clone()), stats.clone(), within);
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let result = rt.block_on(exporter.export(batch));
        (result, inner, stats)
    }

    fn counters(stats: &ExportStats) -> (u64, u64, u64) {
        (
            stats.failures.load(Ordering::SeqCst),
            stats.failed_spans.load(Ordering::SeqCst),
            stats.retries.load(Ordering::SeqCst),
        )
    }

    const FAST: Duration = Duration::ZERO;
    /// Wide enough that a scheduling pause on a loaded host does not push a
    /// zero-length attempt out of it.
    const WINDOW: Duration = Duration::from_millis(250);

    #[test]
    fn a_fast_failure_is_retried_with_the_same_spans() {
        let (result, inner, stats) = export_through(
            &[(FAST, Outcome::InternalFailure), (FAST, Outcome::Ok)],
            WINDOW,
            vec![span("a"), span("b")],
        );

        assert!(
            result.is_ok(),
            "the retry delivered, so the export did: {result:?}"
        );
        let exported = inner.exported.lock().unwrap();
        assert_eq!(
            *exported,
            vec![vec!["a", "b"], vec!["a", "b"]],
            "the retry must carry the batch the first attempt lost"
        );
        assert_eq!(
            counters(&stats),
            (0, 0, 1),
            "(failures, failed_spans, retries)"
        );
    }

    #[test]
    fn a_failure_after_the_window_is_counted_and_not_retried() {
        let (result, inner, stats) = export_through(
            &[(WINDOW * 2, Outcome::InternalFailure)],
            WINDOW,
            vec![span("a"), span("b"), span("c")],
        );

        assert!(result.is_err(), "a lost batch is still an error to the SDK");
        assert_eq!(inner.exported.lock().unwrap().len(), 1, "no second attempt");
        assert_eq!(
            counters(&stats),
            (1, 3, 0),
            "(failures, failed_spans, retries)"
        );
    }

    #[test]
    fn a_timeout_or_shutdown_error_is_counted_and_not_retried() {
        for outcome in [Outcome::Timeout, Outcome::AlreadyShutdown] {
            let (result, inner, stats) =
                export_through(&[(FAST, outcome)], WINDOW, vec![span("a")]);

            assert!(result.is_err(), "{outcome:?}");
            assert_eq!(
                inner.exported.lock().unwrap().len(),
                1,
                "{outcome:?} must not be retried"
            );
            assert_eq!(counters(&stats), (1, 1, 0), "{outcome:?}");
        }
    }

    #[test]
    fn a_retry_that_fails_too_counts_the_batch_once() {
        let (result, inner, stats) = export_through(
            &[
                (FAST, Outcome::InternalFailure),
                (FAST, Outcome::InternalFailure),
            ],
            WINDOW,
            vec![span("a"), span("b")],
        );

        assert!(result.is_err());
        assert_eq!(inner.exported.lock().unwrap().len(), 2);
        assert_eq!(
            counters(&stats),
            (1, 2, 1),
            "(failures, failed_spans, retries)"
        );
    }

    #[test]
    fn the_wrapper_passes_lifecycle_calls_through() {
        let inner = Arc::new(Scripted::default());
        let mut exporter = RetryingExporter::new(
            Shared(inner.clone()),
            Arc::new(ExportStats::default()),
            WINDOW,
        );

        let resource = Resource::builder()
            .with_attribute(KeyValue::new("service.name", "checkout"))
            .build();
        exporter.set_resource(&resource);
        exporter.force_flush().unwrap();
        exporter
            .shutdown_with_timeout(Duration::from_secs(3))
            .unwrap();
        exporter.shutdown().unwrap();

        assert_eq!(
            inner.resource.lock().unwrap().as_ref(),
            Some(&resource),
            "without the resource every exported span loses service.name"
        );
        assert_eq!(inner.flushed.load(Ordering::SeqCst), 1);
        assert_eq!(
            *inner.shut_down.lock().unwrap(),
            vec![Duration::from_secs(3)]
        );
        assert_eq!(inner.plain_shutdowns.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_counters_render_as_prometheus_counters() {
        let stats = ExportStats::default();
        stats.failures.store(2, Ordering::SeqCst);
        stats.failed_spans.store(7, Ordering::SeqCst);
        stats.retries.store(5, Ordering::SeqCst);
        let mut out = String::new();
        stats.collect(&mut out);

        for line in [
            "# TYPE oxphp_otel_export_failures_total counter",
            "oxphp_otel_export_failures_total 2",
            "# TYPE oxphp_otel_export_failed_spans_total counter",
            "oxphp_otel_export_failed_spans_total 7",
            "# TYPE oxphp_otel_export_retries_total counter",
            "oxphp_otel_export_retries_total 5",
        ] {
            assert!(
                out.lines().any(|l| l == line),
                "missing {line:?} in:\n{out}"
            );
        }
    }

    #[test]
    fn the_retry_window_is_half_the_timeout_capped_at_a_second() {
        assert_eq!(
            retry_within(Duration::from_secs(10)),
            Duration::from_secs(1)
        );
        assert_eq!(
            retry_within(Duration::from_millis(500)),
            Duration::from_millis(250)
        );
    }

    // ─── End to end: the real OTLP exporters behind the batch processor ───

    /// The two transports `OTEL_EXPORTER_OTLP_PROTOCOL` selects.
    #[derive(Debug, Clone, Copy)]
    enum Protocol {
        Grpc,
        Http,
    }

    impl Protocol {
        const BOTH: [Protocol; 2] = [Protocol::Grpc, Protocol::Http];

        fn name(self) -> &'static str {
            match self {
                Protocol::Grpc => "grpc",
                Protocol::Http => "http/protobuf",
            }
        }

        /// A collector that accepts every export over this transport, and
        /// counts them.
        async fn ok_server(self) -> (SocketAddr, Arc<AtomicUsize>) {
            match self {
                Protocol::Grpc => grpc_ok_server().await,
                Protocol::Http => {
                    let (addr, calls, _) = http_server(200).await;
                    (addr, calls)
                }
            }
        }
    }

    /// One request the HTTP collector was sent.
    #[derive(Debug)]
    struct Received {
        method: String,
        path: String,
        content_type: String,
        body: Bytes,
    }

    /// An OTLP/HTTP endpoint that answers every request with `status` and an
    /// empty body, records what it was sent, and counts the requests. The
    /// exporter reads nothing from an answer but its status.
    async fn http_server(status: u16) -> (SocketAddr, Arc<AtomicUsize>, Arc<Mutex<Vec<Received>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let received = Arc::new(Mutex::new(Vec::new()));
        let (counted, recorded) = (calls.clone(), received.clone());
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (calls, received) = (counted.clone(), recorded.clone());
                tokio::spawn(async move {
                    let service = service_fn(move |req: hyper::Request<Incoming>| {
                        let (calls, received) = (calls.clone(), received.clone());
                        async move {
                            let method = req.method().to_string();
                            let path = req.uri().path().to_string();
                            let content_type = req
                                .headers()
                                .get("content-type")
                                .and_then(|v| v.to_str().ok())
                                .unwrap_or_default()
                                .to_string();
                            let body = match req.into_body().collect().await {
                                Ok(collected) => collected.to_bytes(),
                                Err(_) => Bytes::new(),
                            };
                            received.lock().unwrap().push(Received {
                                method,
                                path,
                                content_type,
                                body,
                            });
                            calls.fetch_add(1, Ordering::SeqCst);
                            Ok::<_, Infallible>(
                                hyper::Response::builder()
                                    .status(status)
                                    .header("content-type", "application/x-protobuf")
                                    .body(Empty::<Bytes>::new())
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        (addr, calls, received)
    }

    /// A gRPC endpoint that answers every unary call with an empty message and
    /// `grpc-status: 0`, and counts the calls. That is all the OTLP trace
    /// exporter reads from a collector's answer.
    async fn grpc_ok_server() -> (SocketAddr, Arc<AtomicUsize>) {
        grpc_server("0").await
    }

    /// Like [`grpc_ok_server`], answering every call with `grpc-status`.
    async fn grpc_server(status: &'static str) -> (SocketAddr, Arc<AtomicUsize>) {
        grpc_server_with(status, Duration::ZERO).await
    }

    /// Like [`grpc_server`], holding every answer back for `delay`.
    async fn grpc_server_with(
        status: &'static str,
        delay: Duration,
    ) -> (SocketAddr, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = calls.clone();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let calls = counted.clone();
                tokio::spawn(async move {
                    let service = service_fn(move |req: hyper::Request<Incoming>| {
                        let calls = calls.clone();
                        async move {
                            let _ = req.into_body().collect().await;
                            calls.fetch_add(1, Ordering::SeqCst);
                            tokio::time::sleep(delay).await;
                            let mut trailers = http::HeaderMap::new();
                            trailers.insert("grpc-status", http::HeaderValue::from_static(status));
                            let frames = vec![
                                Ok::<_, Infallible>(Frame::data(Bytes::from_static(&[0; 5]))),
                                Ok(Frame::trailers(trailers)),
                            ];
                            let body = StreamBody::new(tokio_stream::iter(frames));
                            Ok::<_, Infallible>(
                                hyper::Response::builder()
                                    .header("content-type", "application/grpc")
                                    .body(body)
                                    .unwrap(),
                            )
                        }
                    });
                    let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                        .serve_connection(TokioIo::new(stream), service)
                        .await;
                });
            }
        });
        (addr, calls)
    }

    /// A TCP proxy that can lose its connections the way a load balancer or a
    /// NAT loses an idle one, and that keeps what each client sent.
    ///
    /// After [`Proxy::forget`], a connection it accepted earlier is reset (RST)
    /// as soon as the client sends anything on it, and the client learns only
    /// then that it is gone. After [`Proxy::freeze`], a connection it accepted
    /// earlier stays open, and once the proxy has read the next chunk the
    /// client sends on it, it neither reads it nor answers on it any more. The
    /// client is never told: once the proxy's receive buffer is full, the
    /// client's system can send nothing more on it. Connections accepted
    /// afterwards are forwarded as usual.
    struct Proxy {
        addr: SocketAddr,
        state: Arc<ProxyState>,
    }

    #[derive(Default)]
    struct ProxyState {
        accepted: AtomicUsize,
        forget_before: AtomicUsize,
        freeze_before: AtomicUsize,
        /// What the client sent on each connection, in the order the
        /// connections were accepted.
        sent: Mutex<Vec<Vec<u8>>>,
    }

    impl Proxy {
        async fn start(upstream: SocketAddr) -> Self {
            Self::start_with_recv_buffer(upstream, None).await
        }

        /// Like [`Proxy::start`], with `SO_RCVBUF` of the connections it
        /// accepts set to `size`.
        async fn start_with_recv_buffer(upstream: SocketAddr, size: Option<u32>) -> Self {
            let socket = tokio::net::TcpSocket::new_v4().unwrap();
            if let Some(size) = size {
                // Set on the listener, before any connection, so the window
                // the accepted ones advertise is sized after it.
                socket.set_recv_buffer_size(size).unwrap();
            }
            socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
            let listener = socket.listen(64).unwrap();
            let addr = listener.local_addr().unwrap();
            let state = Arc::new(ProxyState::default());
            let shared = state.clone();
            tokio::spawn(async move {
                while let Ok((client, _)) = listener.accept().await {
                    let index = shared.accepted.fetch_add(1, Ordering::SeqCst);
                    shared.sent.lock().unwrap().push(Vec::new());
                    tokio::spawn(Self::forward(client, upstream, index, shared.clone()));
                }
            });
            Self { addr, state }
        }

        async fn forward(
            mut client: TcpStream,
            upstream: SocketAddr,
            index: usize,
            state: Arc<ProxyState>,
        ) {
            let Ok(mut server) = TcpStream::connect(upstream).await else {
                return;
            };
            let (mut client_rx, mut client_tx) = client.split();
            let (mut server_rx, mut server_tx) = server.split();
            let mut up = [0u8; 16 * 1024];
            let mut down = [0u8; 16 * 1024];
            loop {
                tokio::select! {
                    read = client_rx.read(&mut up) => {
                        let n = match read {
                            Ok(0) | Err(_) => return,
                            Ok(n) => n,
                        };
                        state.sent.lock().unwrap()[index].extend_from_slice(&up[..n]);
                        if index < state.forget_before.load(Ordering::SeqCst) {
                            // Closing with a zero linger sends RST instead of FIN.
                            let _ = client_rx.as_ref().set_zero_linger();
                            return;
                        }
                        if index < state.freeze_before.load(Ordering::SeqCst) {
                            // Keeps both sockets open, unread, for good.
                            std::future::pending::<()>().await;
                        }
                        if server_tx.write_all(&up[..n]).await.is_err() {
                            return;
                        }
                    }
                    read = server_rx.read(&mut down) => {
                        let n = match read {
                            Ok(0) | Err(_) => return,
                            Ok(n) => n,
                        };
                        if client_tx.write_all(&down[..n]).await.is_err() {
                            return;
                        }
                    }
                }
            }
        }

        fn forget(&self) {
            let accepted = self.accepted();
            self.state.forget_before.store(accepted, Ordering::SeqCst);
        }

        #[cfg(target_os = "linux")]
        fn freeze(&self) {
            let accepted = self.accepted();
            self.state.freeze_before.store(accepted, Ordering::SeqCst);
        }

        fn accepted(&self) -> usize {
            self.state.accepted.load(Ordering::SeqCst)
        }

        /// HTTP/2 PING frames the client sent on connection `index`, leaving
        /// out its acknowledgements of the server's own.
        fn pings(&self, index: usize) -> usize {
            // The client connection preface, then frames, each behind a
            // nine-byte header: length (3), type (1), flags (1), stream (4).
            const PREFACE: usize = 24;
            const PING: u8 = 0x6;
            const ACK: u8 = 0x1;
            let sent = self.state.sent.lock().unwrap();
            let bytes = &sent[index];
            let (mut at, mut pings) = (PREFACE, 0);
            while at + 9 <= bytes.len() {
                let len = u32::from_be_bytes([0, bytes[at], bytes[at + 1], bytes[at + 2]]);
                if bytes[at + 3] == PING && bytes[at + 4] & ACK == 0 {
                    pings += 1;
                }
                at += 9 + len as usize;
            }
            pings
        }
    }

    /// A runtime to host the collector, the proxy and the gRPC channel's own
    /// tasks while the batch processor exports from its own thread.
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap()
    }

    /// Build the plugin's provider exactly as `on_ready` does, exporting over
    /// `protocol` to `endpoint`.
    fn provider_for(
        plugin: &OtelPlugin,
        protocol: Protocol,
        endpoint: SocketAddr,
    ) -> opentelemetry_sdk::trace::SdkTracerProvider {
        provider_with(plugin, protocol, &format!("http://{endpoint}"), None)
    }

    /// Like [`provider_for`], for any endpoint URL and export timeout.
    fn provider_with(
        plugin: &OtelPlugin,
        protocol: Protocol,
        endpoint: &str,
        timeout_ms: Option<&str>,
    ) -> opentelemetry_sdk::trace::SdkTracerProvider {
        let vars = [
            ("OTEL_EXPORTER_OTLP_PROTOCOL", Some(protocol.name())),
            ("OTEL_EXPORTER_OTLP_ENDPOINT", Some(endpoint)),
            ("OTEL_EXPORTER_OTLP_TIMEOUT", timeout_ms),
            ("OTEL_EXPORTER_OTLP_HEADERS", None),
            ("OTEL_TRACES_SAMPLER", Some("always_on")),
        ];
        with_env(&vars, || plugin.init_provider()).expect("provider")
    }

    fn end_span(provider: &opentelemetry_sdk::trace::SdkTracerProvider, name: &'static str) {
        provider.tracer("test").start(name).end();
    }

    #[test]
    fn an_export_over_http_reaches_the_collector() {
        let rt = runtime();
        let (collector, calls, received) = rt.block_on(http_server(200));
        let _guard = rt.enter();
        let plugin = OtelPlugin::new();
        let provider = provider_for(&plugin, Protocol::Http, collector);

        // Two exports in a row: the second goes out only if the batch
        // processor's thread survived the first.
        for (n, name) in [(1, "first"), (2, "second")] {
            end_span(&provider, name);
            let flushed = provider.force_flush();
            assert_eq!(
                calls.load(Ordering::SeqCst),
                n,
                "export {n} never reached the collector (flush: {flushed:?})"
            );
            assert!(flushed.is_ok(), "export {n}: {flushed:?}");
        }
        for (request, name) in received.lock().unwrap().iter().zip(["first", "second"]) {
            assert_eq!(request.method, "POST");
            assert_eq!(
                request.path, "/v1/traces",
                "OTEL_EXPORTER_OTLP_ENDPOINT is a base URL, traces go to v1/traces under it"
            );
            assert_eq!(request.content_type, "application/x-protobuf");
            assert!(
                request
                    .body
                    .windows(name.len())
                    .any(|w| w == name.as_bytes()),
                "span {name} is not in the body of its export"
            );
        }
        assert_eq!(counters(&plugin.export_stats), (0, 0, 0));
        // Shutting down ends the batch processor's thread, which drops the
        // exporter there, and the HTTP client in it joins its own runtime
        // thread as it goes; `shutdown` returns once the processor's thread
        // has ended.
        let shut_down = provider.shutdown();
        assert!(shut_down.is_ok(), "{shut_down:?}");
    }

    #[test]
    fn a_reset_on_a_stale_connection_is_retried_and_delivered() {
        for protocol in Protocol::BOTH {
            let rt = runtime();
            let (collector, calls) = rt.block_on(protocol.ok_server());
            let proxy = rt.block_on(Proxy::start(collector));
            let _guard = rt.enter();
            let plugin = OtelPlugin::new();
            let provider = provider_for(&plugin, protocol, proxy.addr);

            end_span(&provider, "before");
            let flushed = provider.force_flush();
            assert!(flushed.is_ok(), "{protocol:?}: first export: {flushed:?}");
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "{protocol:?}: first export reached the collector"
            );
            assert_eq!(proxy.accepted(), 1, "{protocol:?}");

            // The connection the exporter holds is now dead on the far side,
            // and the exporter does not know it yet.
            proxy.forget();
            end_span(&provider, "after");
            let flushed = provider.force_flush();

            let stats = &plugin.export_stats;
            assert_eq!(
                calls.load(Ordering::SeqCst),
                2,
                "{protocol:?}: the batch exported over the forgotten connection never reached \
                 the collector (flush: {flushed:?})"
            );
            assert_eq!(
                stats.retries.load(Ordering::SeqCst),
                1,
                "{protocol:?}: the export over the forgotten connection did not fail — the \
                 scenario did not happen"
            );
            assert_eq!(
                proxy.accepted(),
                2,
                "{protocol:?}: the retry went out on a new connection"
            );
            assert_eq!(stats.failures.load(Ordering::SeqCst), 0, "{protocol:?}");
            assert!(flushed.is_ok(), "{protocol:?}: {flushed:?}");
            let _ = provider.shutdown();
        }
    }

    #[test]
    fn an_error_status_from_the_collector_is_not_retried() {
        // UNAUTHENTICATED, as for a wrong API key, which the OTLP specification
        // forbids retrying, and UNAVAILABLE, which it allows, after the delay
        // the collector asks for when it names one — a delay the exporter's
        // error does not carry.
        for status in ["16", "14"] {
            let rt = runtime();
            let (collector, calls) = rt.block_on(grpc_server(status));
            let _guard = rt.enter();
            let plugin = OtelPlugin::new();
            let provider = provider_for(&plugin, Protocol::Grpc, collector);

            end_span(&provider, "rejected");
            let flushed = provider.force_flush();

            assert!(flushed.is_err(), "grpc-status {status}: {flushed:?}");
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "the collector answered grpc-status {status} and was sent the batch again"
            );
            assert_eq!(
                counters(&plugin.export_stats),
                (1, 1, 0),
                "grpc-status {status}: (failures, failed_spans, retries)"
            );
            let _ = provider.shutdown();
        }
    }

    #[test]
    fn an_error_status_over_http_is_not_retried() {
        // 401, as for a wrong API key, which the OTLP specification forbids
        // retrying, and 429 and 503, which it allows, after the delay a
        // `Retry-After` header asks for — a header the exporter's error does
        // not carry.
        for status in [401, 429, 503] {
            let rt = runtime();
            let (collector, calls, _) = rt.block_on(http_server(status));
            let _guard = rt.enter();
            let plugin = OtelPlugin::new();
            let provider = provider_for(&plugin, Protocol::Http, collector);

            end_span(&provider, "rejected");
            let flushed = provider.force_flush();

            assert!(flushed.is_err(), "HTTP {status}: {flushed:?}");
            assert_eq!(
                calls.load(Ordering::SeqCst),
                1,
                "the collector answered HTTP {status} and was sent the batch again"
            );
            assert_eq!(
                counters(&plugin.export_stats),
                (1, 1, 0),
                "HTTP {status}: (failures, failed_spans, retries)"
            );
            let _ = provider.shutdown();
        }
    }

    #[test]
    fn an_export_that_fails_twice_is_counted_with_its_spans() {
        for protocol in Protocol::BOTH {
            let rt = runtime();
            // A collector that resets every connection it accepts. It keeps the
            // port, so no other test can take it while this one runs.
            let collector = rt.block_on(async {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                let addr = listener.local_addr().unwrap();
                tokio::spawn(async move {
                    while let Ok((stream, _)) = listener.accept().await {
                        let _ = stream.set_zero_linger();
                    }
                });
                addr
            });
            let _guard = rt.enter();
            let plugin = OtelPlugin::new();
            let provider = provider_for(&plugin, protocol, collector);

            end_span(&provider, "one");
            end_span(&provider, "two");
            let flushed = provider.force_flush();

            assert!(
                flushed.is_err(),
                "{protocol:?}: the SDK still sees the lost batch as an error"
            );
            assert_eq!(
                counters(&plugin.export_stats),
                (1, 2, 1),
                "{protocol:?}: (failures, failed_spans, retries)"
            );
            let _ = provider.shutdown();
        }
    }

    // ─── The channel: connecting, and noticing a connection is gone ──────

    /// Whether `error` comes, at any depth, from an I/O operation that timed out.
    fn timed_out(error: &(dyn std::error::Error + 'static)) -> bool {
        let mut cause = Some(error);
        while let Some(error) = cause {
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() == std::io::ErrorKind::TimedOut)
            {
                return true;
            }
            cause = error.source();
        }
        false
    }

    #[test]
    fn a_connect_that_never_completes_fails_at_the_export_timeout() {
        let rt = runtime();
        // The kernel accepts the connection and nothing ever answers on it, so
        // the TLS handshake waits for a reply that does not come: a collector
        // whose process hangs, or whose accept queue is full, looks like this.
        let silent = rt.block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                let mut held = Vec::new();
                while let Ok((stream, _)) = listener.accept().await {
                    held.push(stream);
                }
            });
            addr
        });
        let timeout = Duration::from_secs(1);
        let mut channel = {
            let _guard = rt.enter();
            // No trust roots: the handshake never gets as far as a certificate.
            grpc_channel(
                format!("https://{silent}"),
                timeout,
                Some(ClientTlsConfig::new()),
            )
            .unwrap()
        };

        let started = Instant::now();
        let answer = rt.block_on(async {
            use tonic::codegen::Service as _;
            let request = http::Request::builder()
                .uri(format!(
                    "https://{silent}/opentelemetry.proto.collector.trace.v1.TraceService/Export"
                ))
                .body(tonic::body::Body::empty())
                .unwrap();
            tokio::time::timeout(timeout * 3, async {
                std::future::poll_fn(|cx| channel.poll_ready(cx)).await?;
                channel.call(request).await
            })
            .await
        });
        let took = started.elapsed();

        let error = answer
            .unwrap_or_else(|_| {
                panic!(
                    "the connect was still waiting after {:?}, three times the export timeout",
                    timeout * 3
                )
            })
            .expect_err("a collector that never answers answered");
        assert!(
            took >= timeout.mul_f64(0.9) && took <= timeout + Duration::from_millis(500),
            "the connect gave up after {took:?}, not at the export timeout of {timeout:?}: {error:?}"
        );
        assert!(
            timed_out(&error),
            "the connect failed for another reason than its timeout: {error:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_connection_that_stops_taking_data_costs_one_export_and_is_replaced() {
        let rt = runtime();
        let (collector, calls) = rt.block_on(grpc_ok_server());
        // Small enough for one large export to fill it.
        let proxy = rt.block_on(Proxy::start_with_recv_buffer(collector, Some(4096)));
        let _guard = rt.enter();
        let plugin = OtelPlugin::new();
        let timeout = Duration::from_secs(2);
        let provider = provider_with(
            &plugin,
            Protocol::Grpc,
            &format!("http://{}", proxy.addr),
            Some("2000"),
        );

        end_span(&provider, "before");
        provider.force_flush().expect("first export");
        assert_eq!(proxy.accepted(), 1);

        // From here on the far side of the exporter's connection takes nothing
        // in and answers nothing, and nothing tells the exporter so: most of
        // the next export stays in its socket, unsent.
        proxy.freeze();
        let tracer = provider.tracer("test");
        tracer
            .span_builder("lost")
            .with_attributes([KeyValue::new("padding", "x".repeat(512 * 1024))])
            .start(&tracer)
            .end();
        let flushed = provider.force_flush();
        assert!(
            flushed.is_err(),
            "an export over a connection that takes nothing succeeded: {flushed:?}"
        );

        // By now the export's bytes have waited longer than the export timeout
        // to go out.
        std::thread::sleep(timeout);
        end_span(&provider, "after");
        let flushed = provider.force_flush();
        assert!(
            flushed.is_ok(),
            "the export after the lost one did not get through: {flushed:?}"
        );
        assert_eq!(
            proxy.accepted(),
            2,
            "the export after the lost one went out on a new connection"
        );
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            counters(&plugin.export_stats),
            (1, 1, 0),
            "(failures, failed_spans, retries)"
        );
        let _ = provider.shutdown();
    }

    #[test]
    fn the_channel_never_pings_the_collector() {
        let rt = runtime();
        // Answers inside the export timeout, after more than half of it.
        let (collector, calls) = rt.block_on(grpc_server_with("0", Duration::from_millis(1500)));
        let proxy = rt.block_on(Proxy::start(collector));
        let _guard = rt.enter();
        let plugin = OtelPlugin::new();
        let provider = provider_with(
            &plugin,
            Protocol::Grpc,
            &format!("http://{}", proxy.addr),
            Some("2000"),
        );

        end_span(&provider, "first");
        provider
            .force_flush()
            .expect("a collector that answers inside the export timeout");
        // Quiet for more than half the export timeout before the next export.
        std::thread::sleep(Duration::from_millis(1500));
        end_span(&provider, "second");
        let flushed = provider.force_flush();

        assert!(flushed.is_ok(), "{flushed:?}");
        // grpc-go, the gRPC server of the OpenTelemetry Collector, counts the
        // pings of a connection across its calls; see `grpc_channel`.
        assert_eq!(proxy.pings(0), 0, "the exporter pinged the collector");
        assert_eq!(proxy.accepted(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(
            counters(&plugin.export_stats),
            (0, 0, 0),
            "(failures, failed_spans, retries)"
        );
        let _ = provider.shutdown();
    }

    /// One integer socket option of `fd`.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn sockopt(fd: std::os::fd::RawFd, level: libc::c_int, name: libc::c_int) -> libc::c_int {
        let mut value: libc::c_int = 0;
        let mut len = std::mem::size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: `fd` is a socket the caller keeps open for the duration of
        // the call, and `value` and `len` describe a buffer of `len` bytes.
        let rc = unsafe {
            libc::getsockopt(
                fd,
                level,
                name,
                (&mut value as *mut libc::c_int).cast(),
                &mut len,
            )
        };
        assert_eq!(
            rc,
            0,
            "getsockopt({level}, {name}): {}",
            std::io::Error::last_os_error()
        );
        value
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn the_channel_socket_carries_its_options() {
        use std::os::fd::AsRawFd;
        use tonic::codegen::Service as _;

        runtime().block_on(async {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let mut connector = grpc_connector(Duration::from_secs(10));
            std::future::poll_fn(|cx| connector.poll_ready(cx))
                .await
                .unwrap();
            let io = connector
                .call(format!("http://{addr}").parse().unwrap())
                .await
                .unwrap();
            let stream = io.inner();
            let fd = stream.as_raw_fd();

            assert!(stream.nodelay().unwrap(), "TCP_NODELAY");
            assert_ne!(
                sockopt(fd, libc::SOL_SOCKET, libc::SO_KEEPALIVE),
                0,
                "SO_KEEPALIVE"
            );
            #[cfg(target_os = "linux")]
            let idle = libc::TCP_KEEPIDLE;
            #[cfg(target_os = "macos")]
            let idle = libc::TCP_KEEPALIVE;
            assert_eq!(sockopt(fd, libc::IPPROTO_TCP, idle), 30, "keepalive idle");
            assert_eq!(
                sockopt(fd, libc::IPPROTO_TCP, libc::TCP_KEEPINTVL),
                10,
                "keepalive interval"
            );
            assert_eq!(
                sockopt(fd, libc::IPPROTO_TCP, libc::TCP_KEEPCNT),
                3,
                "keepalive probes"
            );
            #[cfg(target_os = "linux")]
            assert_eq!(
                sockopt(fd, libc::IPPROTO_TCP, libc::TCP_USER_TIMEOUT),
                10_000,
                "TCP_USER_TIMEOUT, in milliseconds"
            );
        });
    }

    #[test]
    fn a_unix_socket_endpoint_is_refused_at_startup() {
        let rt = runtime();
        let _guard = rt.enter();
        let vars = [
            ("OTEL_EXPORTER_OTLP_PROTOCOL", Some("grpc")),
            (
                "OTEL_EXPORTER_OTLP_ENDPOINT",
                Some("unix:/run/collector.sock"),
            ),
        ];
        let built = with_env(&vars, || OtelPlugin::new().init_provider());

        let Err(PluginError::Config(message)) = built else {
            panic!("a unix: endpoint built an exporter, which sends to no socket");
        };
        assert!(
            message.contains("unix:/run/collector.sock"),
            "the error does not name the endpoint: {message}"
        );
    }
}
