use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Instant;

use bytes::Bytes;
use http::{header, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Limited};
use hyper::body::Body as _;
use hyper::body::Incoming;

use crate::bridge::cancel::{CancelReason, CancellationState};
use crate::events::{RequestComplete, RequestReceived, ResponseBuilding};
use crate::executor::ExecuteResult;
use crate::php::worker_registry::cancel_request;
use crate::server::compression;
use crate::server::response::static_file;
use crate::server::routing::RouteResult;
use crate::server::Server;
use crate::types::{full_body, stream_body, ResponseBody, ScriptRequest};

/// Maximum request body size for POST/PUT/PATCH (10 MB).
const MAX_REQUEST_BODY: usize = 10 * 1024 * 1024;

/// Returns true if the method defines semantics for a request body.
/// Used in tests; hot path in dispatch_request inlines the check to reuse `is_query`.
#[cfg(test)]
fn method_expects_body(method: &Method) -> bool {
    matches!(
        *method,
        Method::POST | Method::PUT | Method::PATCH | Method::DELETE
    ) || is_query_method(method)
}

/// Returns true if the method is QUERY (RFC 10008).
fn is_query_method(method: &Method) -> bool {
    method.as_str() == "QUERY"
}

/// True when `method` is QUERY but the request carries no `Content-Type`.
/// Per RFC 10008 §4 such a request lacks media-type information and is
/// malformed, so it is rejected with a 4xx (we use 400 Bad Request). 415
/// (Unsupported Media Type) is reserved for a Content-Type that is present
/// but unsupported, not for a missing one.
fn query_lacks_content_type(method: &Method, headers: &http::HeaderMap) -> bool {
    is_query_method(method) && !headers.contains_key(http::header::CONTENT_TYPE)
}

/// Look up a key in the metadata vector, returning empty string if not found.
#[inline]
fn metadata_get<'a>(metadata: &'a [(String, String)], key: &str) -> &'a str {
    metadata
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
        .unwrap_or("")
}

/// Parse Content-Length from raw bytes without UTF-8 validation.
/// Content-Length is always ASCII digits — no need for `to_str()` + `parse()`.
fn parse_content_length(bytes: &[u8]) -> Option<usize> {
    if bytes.is_empty() || bytes.len() > 20 {
        return None;
    }
    let mut n: usize = 0;
    for &b in bytes {
        let d = b.wrapping_sub(b'0');
        if d > 9 {
            return None;
        }
        n = n.checked_mul(10)?.checked_add(d as usize)?;
    }
    Some(n)
}

/// Drop-guard that fires `cancel_request(state, ClientAbort)` if the
/// dispatch future is dropped before completing. Disarmed via
/// `disarm()` once the future has returned a result.
struct ClientAbortGuard {
    state: std::sync::Arc<CancellationState>,
}

impl ClientAbortGuard {
    fn new(state: std::sync::Arc<CancellationState>) -> Self {
        Self { state }
    }

    fn disarm(self) {
        self.state.mark_done();
        std::mem::forget(self);
    }
}

impl Drop for ClientAbortGuard {
    fn drop(&mut self) {
        if self.state.is_done() {
            return;
        }
        cancel_request(&self.state, CancelReason::ClientAbort);
    }
}

/// Handle a single HTTP request with event-driven pipeline.
///
/// `closed_rx` is signalled by the owning connection when hyper's
/// `serve_connection` returns. Racing the dispatch against it lets us
/// cancel in-flight workers on HTTP/2 stream resets and HTTP/1.1
/// between-request closes. HTTP/1.1 mid-handler closes for buffered
/// responses remain undetectable here — hyper does not poll the socket
/// while a service handler is running.
pub async fn handle_request(
    req: Request<Incoming>,
    server: std::sync::Arc<Server>,
    remote_addr: SocketAddr,
    mut closed_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<Response<ResponseBody>, Infallible> {
    let start = Instant::now();
    let (parts, body) = req.into_parts();

    // Check brotli support before parts are consumed by the pipeline (no alloc)
    let supports_brotli = server.compression_level > 0
        && parts
            .headers
            .get(http::header::ACCEPT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .is_some_and(compression::accepts_brotli);

    // ── RequestReceived event ──
    // Handlers: RequestIdGenerator (-100), TrustedProxyHandler (-80),
    //           RateLimitHandler (-50), MetricsRequestHandler (0)
    let mut received_event = RequestReceived {
        parts,
        remote_addr,
        request_id: String::new(),
        early_response: None,
        // Pre-allocate: traceparent + trace_id + span_id + parent_span_id + trace_flags
        // + tracestate + peer_addr + forwarded_proto + forwarded_host + forwarded_port
        // ≈ 10 entries
        metadata: Vec::with_capacity(11),
        profiling_mode: None,
        profiling_run_id: None,
    };
    server.dispatcher.dispatch(&mut received_event);

    // Read back remote_addr — TrustedProxyHandler may have overwritten it
    // with the real client IP extracted from Forwarded / X-Forwarded-For.
    let remote_addr = received_event.remote_addr;

    // Take ownership — no clone
    let request_id = std::mem::take(&mut received_event.request_id);
    let metadata = std::mem::take(&mut received_event.metadata);
    let profiling_mode = received_event.profiling_mode;
    let profiling_run_id = std::mem::take(&mut received_event.profiling_run_id);

    // Check for early response (e.g., 429 from rate limiter)
    if let Some(early_resp) = received_event.early_response {
        let status = early_resp.status().as_u16();
        let response_size = early_resp.body().size_hint().exact().unwrap_or(0);
        let elapsed = start.elapsed();

        // Dispatch RequestComplete for the early response
        let mut complete_event = RequestComplete {
            request_id,
            method: received_event.parts.method.clone(),
            path: received_event.parts.uri.path().to_string(),
            status,
            duration: elapsed,
            remote_addr,
            request_body_size: 0,
            response_size,
            metadata,
            php_errors: Vec::new(),
            profile_tree: None,
            queue_wait_us: None,
            php_exec_us: None,
        };
        server.dispatcher.dispatch(&mut complete_event);

        return Ok(early_resp);
    }

    let mut parts = received_event.parts;
    // Clone method (cheap enum copy) and path before parts are consumed
    let method = parts.method.clone();
    let path_str = parts.uri.path().to_string();
    crate::plugin::cookies::strip_plugin_cookies(&mut parts);

    // Per-request cancellation state. Worker holds one Arc (stashed in its
    // TLS slot); this scope holds the other through the ClientAbortGuard,
    // so the byte the bridge reads is alive even if either side drops first.
    let cancel_state = std::sync::Arc::new(CancellationState::new());

    // Race the dispatch against the connection-closed watch in an inner
    // scope so the pinned dispatch future (which borrows `request_id`,
    // `metadata`, `server`) is fully dropped before those values are moved
    // into `RequestComplete` / `ResponseBuilding` below.
    let result = {
        let dispatch = dispatch_request(
            parts,
            body,
            &server,
            remote_addr,
            &request_id,
            &metadata,
            profiling_mode,
            profiling_run_id,
            cancel_state.clone(),
            supports_brotli,
        );

        // Drop guard fires cancel_request(ClientAbort) if the dispatch future
        // is dropped before completing (hyper saw the client go away). Disarmed
        // on the success path. Declared after `dispatch` so on a future-drop
        // its Drop runs after the dispatch local has already gone.
        let guard = ClientAbortGuard::new(cancel_state);

        // If the connection ends first (HTTP/2 stream RST, HTTP/1.1 close
        // between requests, or any other serve_connection completion), drop
        // the guard to fire cancel_request(ClientAbort) on the worker, then
        // keep awaiting the dispatch so the worker can ship its (likely
        // 499 / 500) response.
        tokio::pin!(dispatch);
        tokio::select! {
            biased;
            r = &mut dispatch => {
                guard.disarm();
                r
            }
            _ = closed_rx.changed() => {
                // Fires cancel_request(ClientAbort) via Drop on the worker side.
                drop(guard);
                // Wait for the worker to actually finish so we get a real
                // response back instead of synthesising one and racing the
                // worker.
                dispatch.await
            }
        }
    };

    let (response, request_body_size, mut php_exec) = match result {
        Ok((resp, body_size, exec)) => (resp, body_size, exec),
        Err(e) => {
            tracing::error!(error = %e, path = %path_str, request_id = %request_id, "Internal server error");
            (
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                    .body(full_body(Bytes::from_static(b"500 Internal Server Error")))
                    .unwrap(),
                0,
                PhpExecData::default(),
            )
        }
    };

    // ── ResponseBuilding event ──
    // Handlers: TraceContextResponseHandler (-95), ErrorPagesHandler (60),
    // ServerHeaderHandler (100), SecurityHeadersHandler (100).
    // ErrorPagesHandler must run before SecurityHeadersHandler: a custom error
    // page replaces the response (dropping app headers), and the security
    // fallbacks are re-applied only because they run later.
    let mut building_event = ResponseBuilding {
        request_id, // move in (handlers only read &str)
        response,
        metadata,
    };
    server.dispatcher.dispatch(&mut building_event);
    let response = building_event.response;
    let request_id = building_event.request_id; // move back out
    let metadata = std::mem::take(&mut building_event.metadata);

    // ── Brotli compression (after error pages, before metrics/logging) ──
    let response = if supports_brotli {
        let pre_size = response.body().size_hint().exact().unwrap_or(0);
        let compressed = compression::maybe_compress(response, server.compression_level).await;
        let post_size = compressed.body().size_hint().exact().unwrap_or(0);
        if post_size < pre_size {
            server.metrics.record_compression(pre_size - post_size);
        }
        compressed
    } else {
        response
    };

    // ── RequestComplete event ──
    // Handlers: MetricsResponseHandler (0), AccessLogHandler (100)
    let status = response.status().as_u16();
    let response_size = response.body().size_hint().exact().unwrap_or(0);

    // Defer RequestComplete only for a streaming response that already committed a
    // 5xx: its terminal fatal (thrown after the headers went out) arrives when the
    // stream closes, and the root-span exception event needs it. A response below
    // 500 cannot be re-flagged by a late error (documented streaming boundary), so
    // it dispatches inline below — restoring immediate access-log / metrics for a
    // long-lived 2xx SSE instead of withholding them until the stream ends. The
    // deferred task holds a drain guard so graceful shutdown waits for it (and for
    // the synchronous span build inside the completion handler) before flushing.
    if status >= 500 {
        if let Some(late_errors_rx) = php_exec.late_errors_rx.take() {
            let profile_tree = php_exec.profile_tree.take();
            let queue_wait_us = php_exec.queue_wait_us;
            let php_exec_us = php_exec.php_exec_us;
            let drain_guard = server.begin_deferred_completion();
            tokio::spawn(async move {
                let _drain_guard = drain_guard;
                // Err = the sender was dropped (client vanished before the stream
                // closed); fall back to no late errors.
                let php_errors = late_errors_rx.await.unwrap_or_default();
                let mut complete_event = RequestComplete {
                    request_id,
                    method,
                    path: path_str,
                    status,
                    duration: start.elapsed(),
                    remote_addr,
                    request_body_size: request_body_size as u64,
                    response_size,
                    metadata,
                    php_errors,
                    profile_tree,
                    queue_wait_us,
                    php_exec_us,
                };
                server.dispatcher.dispatch(&mut complete_event);
            });
            return Ok(response);
        }
    }

    let elapsed = start.elapsed();
    let mut complete_event = RequestComplete {
        request_id, // move — no clone
        method,
        path: path_str,
        status,
        duration: elapsed,
        remote_addr,
        request_body_size: request_body_size as u64,
        response_size,
        metadata,
        php_errors: std::mem::take(&mut php_exec.php_errors),
        profile_tree: php_exec.profile_tree.take(),
        queue_wait_us: php_exec.queue_wait_us,
        php_exec_us: php_exec.php_exec_us,
    };
    server.dispatcher.dispatch(&mut complete_event);

    Ok(response)
}

/// Typed data produced by PHP execution and propagated to `RequestComplete`.
/// Static-file and error paths leave all fields at defaults.
#[derive(Default)]
struct PhpExecData {
    php_errors: Vec<crate::types::PhpScriptError>,
    /// Streaming only: delivers the final `php_errors` when the stream closes.
    /// When present, `RequestComplete` is deferred until it resolves so a fatal
    /// thrown after the 5xx headers still reaches the root span.
    late_errors_rx: Option<tokio::sync::oneshot::Receiver<Vec<crate::types::PhpScriptError>>>,
    profile_tree: Option<std::sync::Arc<crate::profiling::SpanTree>>,
    queue_wait_us: Option<u64>,
    php_exec_us: Option<u64>,
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_request(
    parts: http::request::Parts,
    body: Incoming,
    server: &Server,
    remote_addr: SocketAddr,
    request_id: &str,
    metadata: &[(String, String)],
    profiling_mode_override: Option<crate::profiling::ProfilingMode>,
    profiling_run_id: Option<String>,
    cancel_state: std::sync::Arc<crate::bridge::cancel::CancellationState>,
    supports_brotli: bool,
) -> Result<(Response<ResponseBody>, usize, PhpExecData), crate::types::BoxError> {
    let uri_path = parts.uri.path();
    let route_result = server
        .route_config
        .resolve_request(uri_path, &server.file_cache)
        .await;

    let mut request_body_size = 0usize;

    let (response, exec_data) = match &*route_result {
        RouteResult::Serve(file_path) => {
            // Read-only cache check: read lock, no LRU update, no stat() syscall
            let cache_key = file_path.to_string_lossy();
            let was_cached = server.file_cache.content_cached(&cache_key);

            let response = static_file::serve(
                file_path,
                &server.file_cache,
                server.route_config.canonical_root(),
                server.route_config.symlink_allow(),
                &parts.method,
                &parts.headers,
                server.static_cache_control.as_deref(),
                supports_brotli,
            )
            .await?;

            if was_cached {
                server.metrics.static_cache_hit();
            } else {
                server.metrics.static_cache_miss();
            }

            (response, PhpExecData::default())
        }
        RouteResult::Execute(script_path, path_info, denied_meta) => {
            let script_path = script_path.clone();
            let path_info = path_info.clone();
            let denied_meta = denied_meta.clone();
            if denied_meta.is_some() {
                server.metrics.php_denied();
            }
            let is_query = is_query_method(&parts.method);

            // RFC 10008 §4: a QUERY request without media-type information is
            // malformed. Reject with 400 (Bad Request) — 415 (Unsupported Media
            // Type) is reserved for a Content-Type that is present but unsupported.
            if query_lacks_content_type(&parts.method, &parts.headers) {
                return Ok((
                    Response::builder()
                        .status(StatusCode::BAD_REQUEST)
                        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                        .body(full_body(Bytes::from_static(
                            b"400 Bad Request: QUERY requires a Content-Type",
                        )))?,
                    0,
                    PhpExecData::default(),
                ));
            }

            // Collect body for methods that carry a payload.
            // QUERY uses a separate configurable limit (MAX_QUERY_BODY, default 512 KB).
            let body_bytes = if is_query
                || matches!(
                    parts.method,
                    Method::POST | Method::PUT | Method::PATCH | Method::DELETE
                ) {
                let limit = if is_query {
                    server.max_query_body
                } else {
                    MAX_REQUEST_BODY
                };

                // Early rejection via Content-Length header — zero I/O, no body read
                if let Some(cl) = parts.headers.get(http::header::CONTENT_LENGTH) {
                    if parse_content_length(cl.as_bytes()).is_some_and(|len| len > limit) {
                        return Ok((
                            Response::builder()
                                .status(StatusCode::PAYLOAD_TOO_LARGE)
                                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                                .body(full_body(Bytes::from_static(b"413 Payload Too Large")))?,
                            0,
                            PhpExecData::default(),
                        ));
                    }
                }

                // Streaming limit — safety net for chunked transfers or lying Content-Length
                let limited = Limited::new(body, limit);
                match BodyExt::collect(limited).await {
                    Ok(collected) => collected.to_bytes(),
                    Err(e) => {
                        if e.downcast_ref::<http_body_util::LengthLimitError>()
                            .is_some()
                        {
                            return Ok((
                                Response::builder()
                                    .status(StatusCode::PAYLOAD_TOO_LARGE)
                                    .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                                    .body(full_body(Bytes::from_static(
                                        b"413 Payload Too Large",
                                    )))?,
                                0,
                                PhpExecData::default(),
                            ));
                        }
                        return Err(e);
                    }
                }
            } else {
                Bytes::new()
            };

            request_body_size = body_bytes.len();
            let query_string = parts.uri.query().unwrap_or("").to_string();

            // Profiling mode fallback: if no plugin opted in, default to
            // ApmOnly when the APM plugin is compiled in (preserves pre-PR
            // behaviour) and Off otherwise. NB: `plugin-profiler` being in
            // the default feature set does not change this default —
            // profiler runs are opt-in per request via trigger
            // (bearer/header/cookie/sample), so an always-on default would
            // silently profile every request. The profiler plugin upgrades
            // the mode to ProfileAll inside on_request_start when a trigger
            // matches.
            let profiling_mode = profiling_mode_override.unwrap_or({
                #[cfg(feature = "plugin-apm")]
                {
                    crate::profiling::ProfilingMode::ApmOnly
                }
                #[cfg(not(feature = "plugin-apm"))]
                {
                    crate::profiling::ProfilingMode::Off
                }
            });

            let script_request = ScriptRequest {
                request_id: request_id.to_string(),
                script_path,
                method: parts.method,
                uri: parts.uri,
                query_string,
                headers: parts.headers,
                body: body_bytes,
                remote_addr,
                document_root: server.route_config.document_root_arc(),
                cancel_state,
                trace_id: metadata_get(metadata, "trace_id").to_string(),
                span_id: metadata_get(metadata, "span_id").to_string(),
                parent_span_id: metadata_get(metadata, "parent_span_id").to_string(),
                is_tls: server.is_tls(),
                version: parts.version,
                path_info,
                forwarded_proto: metadata
                    .iter()
                    .find(|(k, _)| k == "forwarded_proto")
                    .map(|(_, v)| v.clone()),
                forwarded_host: metadata
                    .iter()
                    .find(|(k, _)| k == "forwarded_host")
                    .map(|(_, v)| v.clone()),
                forwarded_port: metadata
                    .iter()
                    .find(|(k, _)| k == "forwarded_port")
                    .and_then(|(_, v)| v.parse::<u16>().ok()),
                denied_meta,
                profiling_mode,
                profiling_run_id: profiling_run_id.clone(),
            };

            let queue_start = Instant::now();
            server.metrics.request_queued();
            let execute_result = server.executor.execute(script_request);

            let (mut script_response, exec_data) = match execute_result {
                ExecuteResult::Immediate(resp) => {
                    server.metrics.request_dequeued();
                    let queue_wait_us = queue_start.elapsed().as_micros() as u64;
                    server.metrics.record_queue_wait(queue_wait_us);
                    let php_exec_us = resp.execution_time_us;
                    (
                        resp,
                        PhpExecData {
                            queue_wait_us: Some(queue_wait_us),
                            php_exec_us: Some(php_exec_us),
                            ..PhpExecData::default()
                        },
                    )
                }
                ExecuteResult::Deferred(rx) => match rx.await {
                    Ok(resp) => {
                        server.metrics.request_dequeued();
                        let queue_wait_us = queue_start.elapsed().as_micros() as u64;
                        server.metrics.record_queue_wait(queue_wait_us);
                        let php_exec_us = resp.execution_time_us;
                        (
                            resp,
                            PhpExecData {
                                queue_wait_us: Some(queue_wait_us),
                                php_exec_us: Some(php_exec_us),
                                ..PhpExecData::default()
                            },
                        )
                    }
                    Err(_) => {
                        server.metrics.request_dropped();
                        return Ok((
                            Response::builder()
                                .status(StatusCode::INTERNAL_SERVER_ERROR)
                                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                                .body(full_body(Bytes::from_static(b"500 PHP Worker Error")))
                                .unwrap(),
                            request_body_size,
                            PhpExecData::default(),
                        ));
                    }
                },
            };

            // Bump the per-reason cancellation counter once per request,
            // observed from the worker-side reason mirrored on the
            // ScriptResponse. 0 = no cancellation → no-op.
            server
                .metrics
                .observe_cancelled(script_response.cancel_reason);

            // Graceful-drain replies (Shutdown → 503) advertise a short
            // retry window so clients can hit a recovered/replacement
            // instance. Userland-set Retry-After wins.
            if script_response.cancel_reason == CancelReason::Shutdown as u8
                && script_response.status == 503
                && !script_response
                    .headers
                    .iter()
                    .any(|(n, _)| n == header::RETRY_AFTER)
            {
                script_response
                    .headers
                    .push((header::RETRY_AFTER, http::HeaderValue::from_static("5")));
            }

            // Move PHP errors and profile tree into typed exec data.
            let exec_data = PhpExecData {
                php_errors: std::mem::take(&mut script_response.errors),
                late_errors_rx: script_response.late_errors_rx.take(),
                profile_tree: script_response.profile_tree.take(),
                ..exec_data
            };

            let mut builder = Response::builder().status(script_response.status);
            for (name, value) in &script_response.headers {
                builder = builder.header(name, value);
            }
            let response = if let Some(rx) = script_response.stream_rx {
                builder.body(stream_body(script_response.body, rx)).unwrap()
            } else {
                builder.body(full_body(script_response.body)).unwrap()
            };
            (response, exec_data)
        }
        RouteResult::NotFound => (
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(full_body(Bytes::from_static(b"404 Not Found")))?,
            PhpExecData::default(),
        ),
        RouteResult::Denied(code) => {
            // `Denied` is emitted exclusively by the `PHP_DENY_PATHS`
            // status-fallback path in `routing/traditional.rs`, so the
            // metric increment here is source-specific by construction.
            server.metrics.php_denied();
            (
                Response::builder()
                    .status(
                        StatusCode::from_u16(*code).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    )
                    .body(full_body(Bytes::new()))?,
                PhpExecData::default(),
            )
        }
    };

    Ok((response, request_body_size, exec_data))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::events::EventHandler;
    use crate::handlers::request_id::RequestIdGenerator;

    #[test]
    fn test_method_expects_body_standard() {
        assert!(method_expects_body(&Method::POST));
        assert!(method_expects_body(&Method::PUT));
        assert!(method_expects_body(&Method::PATCH));
        assert!(method_expects_body(&Method::DELETE));
    }

    #[test]
    fn test_method_expects_body_query() {
        let query = Method::from_bytes(b"QUERY").unwrap();
        assert!(method_expects_body(&query));
        assert!(is_query_method(&query));
    }

    #[test]
    fn test_method_no_body() {
        assert!(!method_expects_body(&Method::GET));
        assert!(!method_expects_body(&Method::HEAD));
        assert!(!method_expects_body(&Method::OPTIONS));
        assert!(!method_expects_body(&Method::TRACE));
    }

    #[test]
    fn test_query_lacks_content_type() {
        let query = Method::from_bytes(b"QUERY").unwrap();

        // QUERY without Content-Type → malformed (dispatch returns 400).
        let empty = http::HeaderMap::new();
        assert!(query_lacks_content_type(&query, &empty));

        // QUERY with Content-Type → accepted.
        let mut with_ct = http::HeaderMap::new();
        with_ct.insert(
            http::header::CONTENT_TYPE,
            http::HeaderValue::from_static("application/sql"),
        );
        assert!(!query_lacks_content_type(&query, &with_ct));

        // Non-QUERY methods are never rejected by this check, even without a type.
        assert!(!query_lacks_content_type(&Method::POST, &empty));
        assert!(!query_lacks_content_type(&Method::GET, &empty));
    }

    #[test]
    fn test_query_method_case_sensitive() {
        let lowercase = Method::from_bytes(b"query").unwrap();
        assert!(!is_query_method(&lowercase));
        assert!(!method_expects_body(&lowercase));

        let mixed = Method::from_bytes(b"Query").unwrap();
        assert!(!is_query_method(&mixed));
    }

    #[test]
    fn test_parse_content_length() {
        assert_eq!(parse_content_length(b"0"), Some(0));
        assert_eq!(parse_content_length(b"123"), Some(123));
        assert_eq!(parse_content_length(b"524288"), Some(524288));
        assert_eq!(parse_content_length(b"10485760"), Some(10_485_760));
        assert_eq!(parse_content_length(b""), None);
        assert_eq!(parse_content_length(b"abc"), None);
        assert_eq!(parse_content_length(b"12 34"), None);
        assert_eq!(parse_content_length(b"-1"), None);
        // 21 digits — exceeds max length guard
        assert_eq!(parse_content_length(b"123456789012345678901"), None);
    }

    #[test]
    fn test_request_id_generation() {
        let handler = RequestIdGenerator;
        let (parts, _) = http::Request::builder()
            .method(http::Method::GET)
            .uri("/test")
            .body(())
            .unwrap()
            .into_parts();

        let mut event = RequestReceived {
            parts,
            remote_addr: SocketAddr::new(std::net::Ipv4Addr::new(127, 0, 0, 1).into(), 8080),
            request_id: String::new(),
            early_response: None,
            metadata: Vec::new(),
            profiling_mode: None,
            profiling_run_id: None,
        };

        handler.handle(&mut event);
        assert_eq!(event.request_id.len(), 20);
        assert!(event.request_id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_request_id_uniqueness() {
        let handler = RequestIdGenerator;
        let ids: Vec<String> = (0..100)
            .map(|_| {
                let (parts, _) = http::Request::builder()
                    .method(http::Method::GET)
                    .uri("/test")
                    .body(())
                    .unwrap()
                    .into_parts();

                let mut event = RequestReceived {
                    parts,
                    remote_addr: SocketAddr::new(
                        std::net::Ipv4Addr::new(127, 0, 0, 1).into(),
                        8080,
                    ),
                    request_id: String::new(),
                    early_response: None,
                    metadata: Vec::new(),
                    profiling_mode: None,
                    profiling_run_id: None,
                };

                handler.handle(&mut event);
                event.request_id
            })
            .collect();

        let mut deduped = ids.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(ids.len(), deduped.len());
    }
}
