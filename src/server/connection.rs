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
use crate::metrics::Metrics;
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
///
/// It is also where `oxphp_request_cancelled_total` is incremented, once per
/// guard and for whatever reason the cell ends up holding. The guard is the
/// one thing a cancelled request cannot get past: `disarm()` takes `self`, so
/// `Drop` runs on the path where the dispatch returned normally too, and the
/// reason it reads there is whatever the worker stored. And where a client
/// abort takes the request future out from under the dispatch, the dispatch
/// never returns at all — so counting anywhere downstream of it misses
/// exactly the case the counter exists for.
///
/// What bounds it is the dispatch scope, not the response. A response sent
/// early — a stream, or `oxphp_finish_request()` — leaves that scope while
/// the handler is still running, so a cancellation raised afterwards (the
/// stream's own `blocking_send` failure, say) is read by nothing: not here,
/// the guard having already gone, and nowhere else either. That gap is older
/// than this guard — the site that used to count read the same zero off the
/// early-sent response — and is tracked on its own.
struct ClientAbortGuard<'a> {
    state: std::sync::Arc<CancellationState>,
    metrics: &'a Metrics,
    /// The reason carried by the answer this request got, where it got one.
    answered: Option<u8>,
}

impl<'a> ClientAbortGuard<'a> {
    fn new(state: std::sync::Arc<CancellationState>, metrics: &'a Metrics) -> Self {
        Self {
            state,
            metrics,
            answered: None,
        }
    }

    /// `mark_done()` IS the disarm — `Drop` below stops cancelling on the
    /// flag. It still counts: a request the worker cancelled itself
    /// (`Timeout`, `Shutdown`) comes back through here.
    ///
    /// What it counts is `answered` — the reason on the response that came
    /// back — and not the cell as it reads at this moment. The two differ
    /// for a window the worker opens on every request: it publishes its
    /// response and only then unregisters, so a drain sweep arriving between
    /// the two writes into the cell of a request that has already been
    /// answered `200`, and reading the cell here would file that request
    /// under `shutdown`. The response is also simply the better authority —
    /// it is what the client got, and what the status beside it was chosen
    /// from.
    ///
    /// `None` means no response at all reached this scope: a static file or
    /// a `404`, which never had a cell to fill, or a worker whose channel
    /// was dropped out from under the request, where the cell is the only
    /// account of what happened. Those fall through to it.
    fn disarm(mut self, answered: Option<u8>) {
        self.state.mark_done();
        self.answered = answered;
    }
}

impl Drop for ClientAbortGuard<'_> {
    fn drop(&mut self) {
        if !self.state.is_done() {
            cancel_request(&self.state, CancelReason::ClientAbort);
        }
        // `None` is the ordinary case and `observe_cancelled` ignores it.
        let reason = self.answered.unwrap_or_else(|| self.state.get() as u8);
        self.metrics.observe_cancelled(reason);
    }
}

/// Handle a single HTTP request with event-driven pipeline.
///
/// `closed_rx` is signalled by the owning connection when hyper's
/// `serve_connection` returns. Racing the dispatch against it lets us
/// cancel in-flight workers on HTTP/2 stream resets and HTTP/1.1
/// between-request closes.
///
/// A close that arrives while this handler is still running is not one of
/// those, and needs nothing from this select: hyper keeps reading the socket
/// for exactly that case, so an EOF mid-message ends the connection and drops
/// this future with it, which fires the guard below.
///
/// That rests on hyper having nothing pending on the request body — it reads
/// the socket only once the body is done with. `dispatch_request` collects the
/// bodies it expects before dispatching, so by then there is nothing left to
/// read; a message carrying a body this server never reads (a `GET` with one,
/// or a method it does not buffer) parks hyper on the body channel instead,
/// and the close is not seen until the handler returns. Nor is it seen while a
/// pipelined next request sits unread in hyper's buffer.
///
/// A client that stops waiting without closing — a dropped route, or a caller
/// that abandons the response and holds the socket open — sends nothing to
/// detect on any protocol, and its request runs to completion for nobody.
pub async fn handle_request(
    req: Request<Incoming>,
    server: &Server,
    remote_addr: SocketAddr,
    mut closed_rx: tokio::sync::watch::Receiver<bool>,
) -> Result<Response<ResponseBody>, Infallible> {
    let start = Instant::now();
    let (parts, body) = req.into_parts();

    // Weigh Accept-Encoding before parts are consumed by the pipeline, and
    // without allocating for the one field line every client actually sends.
    // Which coding wins depends on where the bytes are headed, so the weights
    // are kept and resolved separately on each path.
    let accepted = compression::Acceptable::from_headers(&parts.headers, server.compression);

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
            server,
            remote_addr,
            &request_id,
            &metadata,
            profiling_mode,
            profiling_run_id,
            cancel_state.clone(),
            accepted.artifact(),
        );

        // Drop guard fires cancel_request(ClientAbort) if the dispatch future
        // is dropped before completing (hyper saw the client go away). Disarmed
        // on the success path. Declared after `dispatch` so on a future-drop
        // its Drop runs after the dispatch local has already gone.
        let guard = ClientAbortGuard::new(cancel_state, &server.metrics);

        // If the connection ends first (HTTP/2 stream RST, HTTP/1.1 close
        // between requests, or any other serve_connection completion), drop
        // the guard to fire cancel_request(ClientAbort) on the worker, then
        // keep awaiting the dispatch so the worker can ship its (likely
        // 499 / 500) response.
        tokio::pin!(dispatch);
        tokio::select! {
            biased;
            r = &mut dispatch => {
                guard.disarm(r.as_ref().ok().and_then(|(_, _, exec)| exec.cancel_reason));
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

    // ── Compression (after error pages, before metrics/logging) ──
    let mut response = response;
    if let Some(wanted) = response
        .extensions_mut()
        .remove::<static_file::ArtifactWanted>()
    {
        schedule_artifact(server, wanted);
    }
    let response = if let Some(saving) = response
        .extensions()
        .get::<static_file::PrecompressedSaving>()
        .copied()
    {
        // Body is already a cached artifact — nothing left to compress, and the
        // saving is only knowable where the identity length was.
        server.metrics.record_compression(saving.0 as u64);
        response
    } else if let Some(coding) = accepted.per_request() {
        let pre_size = response.body().size_hint().exact().unwrap_or(0);
        let compressed =
            compression::maybe_compress(response, coding, server.compression.level(coding)).await;
        let post_size = compressed.body().size_hint().exact().unwrap_or(0);
        if post_size < pre_size {
            server.metrics.record_compression(pre_size - post_size);
        }
        compressed
    } else {
        response
    };

    // Whatever happened above, a body this server would compress for some
    // client varies by Accept-Encoding — including the identity copy this one
    // is getting. Without the header a shared cache stores that copy under the
    // bare URL and serves it to clients that would have taken a compressed one.
    let mut response = response;
    compression::mark_varies_by_encoding(&mut response, server.compression);
    let response = response;

    // ── RequestComplete event ──
    // Handlers: MetricsResponseHandler (0), AccessLogHandler (100)
    let status = response.status().as_u16();
    let response_size = response.body().size_hint().exact().unwrap_or(0);

    // RequestComplete dispatches synchronously for every response — access log and
    // metrics are never withheld for the lifetime of a long-lived stream. For a
    // streaming or `finish_request()` response, `php_errors` is the snapshot taken
    // when the headers went out (empty at header time for a stream; the rest
    // accumulate on the worker thread and are dropped at teardown). A fatal thrown
    // *after* such a response has already committed its status is therefore not
    // attached to the root span — a documented streaming boundary, symmetric with
    // a sub-500 stream that a late error cannot re-flag. The common non-streaming
    // 5xx (a plain PHP fatal) carries its exception here: its errors are final by
    // send time.
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

/// How many artifacts may be built at once. The build is CPU-bound at a
/// quality chosen for size alone, and it competes with the runtime and the PHP
/// pool for the same cores; a cold cache wants one per distinct file, which on
/// a site with a few hundred assets is a burst large enough to swamp the
/// blocking pool that every `tokio::fs` call on the static path also uses.
/// A quarter of the cores caps that without stalling the build-out: nothing
/// waits on an artifact, so the only cost of a small cap is that the last file
/// gets its stored copy a few seconds later.
static ARTIFACT_BUILDS: std::sync::LazyLock<tokio::sync::Semaphore> =
    std::sync::LazyLock::new(|| {
        let cpu = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        tokio::sync::Semaphore::new((cpu / 4).max(1))
    });

/// Build one coding's artifact for a cached static file, off the request path.
///
/// The compression runs at a quality no request could afford to wait for
/// (tens of milliseconds per file), so it goes to the blocking pool and this
/// request is served the per-request way. Requests that arrive before the job
/// lands are served that way too; the artifact takes over from the first hit
/// after it is stored. A file whose artifact is already being built is skipped
/// rather than queued — the claim is what keeps a cold cache under load from
/// compressing the same file once per concurrent request — and so is one that
/// finds no permit free, since the next hit asks again.
fn schedule_artifact(server: &Server, wanted: static_file::ArtifactWanted) {
    let Ok(permit) = ARTIFACT_BUILDS.try_acquire() else {
        return;
    };
    let Some(claim) = server.file_cache.claim_artifact(&wanted.key, wanted.coding) else {
        return;
    };
    let cache = std::sync::Arc::clone(&server.file_cache);
    tokio::task::spawn_blocking(move || {
        // Both released on every exit, including a panic inside this task.
        let _permit = permit;
        let _claim = claim;
        match compression::compress_artifact(&wanted.bytes, wanted.coding) {
            Some(artifact) => cache.insert_artifact(
                &wanted.key,
                wanted.coding,
                wanted.modified,
                Bytes::from(artifact),
            ),
            // Compression did not shrink these bytes. Recorded so the entry
            // stops asking — otherwise every hit would repeat this work.
            None => cache.reject_artifact(&wanted.key, wanted.coding, wanted.modified),
        }
    });
}

/// Typed data produced by PHP execution and propagated to `RequestComplete`.
/// Static-file and error paths leave all fields at defaults.
#[derive(Default)]
struct PhpExecData {
    php_errors: Vec<crate::types::PhpScriptError>,
    profile_tree: Option<std::sync::Arc<crate::profiling::SpanTree>>,
    queue_wait_us: Option<u64>,
    php_exec_us: Option<u64>,
    /// The cancel reason on the response the pool returned, for the abort
    /// guard to count. `None` on the paths that return no such response.
    cancel_reason: Option<u8>,
}

/// Wait for the pool's answer, and no longer than the request is allowed to
/// wait for one.
///
/// The wait budget is one deadline covering both waits a request can face —
/// for a queue slot, then inside the queue for a worker — stamped on arrival
/// at whatever the budget was worth then, which `QUEUE_WAIT_TIMEOUT_MS` bounds
/// from above. The pool enforces the second half when it takes the request off
/// the channel, which answers it only as early as the next pickup and not at
/// all when no pickup is coming: an application calling back into this same
/// server occupies a worker while it waits for one, so where every worker is
/// held that way the queue cannot move until a request that cannot finish
/// does. The clock on this side is the one that keeps running.
///
/// What the deadline gates is whether a worker has the request, never how long
/// the request has taken: a worker that claimed it first keeps it, whatever it
/// then spends on it. A budget that bounded running handlers would refuse a
/// request a worker started on well inside its deadline, purely for taking
/// longer than the *wait* was allowed to be.
async fn await_queued(
    queued: crate::executor::Queued,
    cancel_state: &CancellationState,
    metrics: &crate::metrics::Metrics,
    starts_on_arrival: u64,
    rejected: &mut bool,
) -> Result<crate::types::ScriptResponse, ()> {
    let crate::executor::Queued {
        mut rx,
        deadline,
        wait_at_ceiling,
    } = queued;
    // Fail-fast mode has no wait to bound, so it arms no timer.
    let Some(deadline) = deadline else {
        return rx.await.map_err(|_| ());
    };

    tokio::select! {
        // Poll the answer first. Nothing rests on it for the race it looks
        // like it settles: a worker holding this request has already taken the
        // claim below, so a timer firing next to a finished response loses
        // that claim and comes back to this same channel. It saves a response
        // that beat the deadline from taking the long way round through a
        // refusal it cannot win. The order does decide one thing — a sender
        // dropped without any claim, as a pool being torn down does, reads
        // here as the worker-error `500` and on the other arm as the
        // deadline's `529` — and for a pool that is gone neither is wrong.
        biased;
        resp = &mut rx => return resp.map_err(|_| ()),
        _ = tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)) => {}
    }

    // Losing the claim means a worker already has the request — running it, or
    // refusing it on this same deadline, having reached it a moment sooner.
    // Either way the answer comes back through the channel, so keep waiting
    // for it rather than adding a second one.
    if !cancel_state.claim_from_queue() {
        return rx.await.map_err(|_| ());
    }

    // Both things are true of this request — its client left and its budget
    // ran out — and only one of them describes what happened. A refusal
    // nobody is waiting for is not one the pool handed out, and
    // `oxphp_admission_refused_total{reason="wait_timeout"}` is read as the
    // case for shortening `QUEUE_WAIT_TIMEOUT_MS`: an argument about the
    // pool, which a client's own patience has no business making. The
    // cancellation is already counted where it happened, in the abort guard
    // that filled this cell.
    //
    // `ClientAbort` and not merely "cancelled": it is the absent client that
    // makes the refusal pointless, and `client_closed()` is the only answer
    // this reasoning licenses. Every other reason is a worker's own, and a
    // request still in the queue has no worker to have written one.
    if cancel_state.get() == CancelReason::ClientAbort {
        // Still set, because what this flag gates is the queue-wait histogram
        // and not the refusal count: no worker picked this request up, so its
        // wait is not a pickup latency whatever the reason it ended.
        *rejected = true;
        return Ok(crate::types::ScriptResponse::client_closed());
    }

    *rejected = true;
    metrics.request_admission_refused(crate::executor::admission::ShedReason::WaitTimeout);
    // Whether this wait ever stood a chance. Unchanged means the pool began
    // no request at all for the whole budget, so this one was not outrun by
    // others — there was no work being started to be outrun by.
    //
    // Only asked of a wait that was given the configured budget. "The pool
    // began nothing while this request waited" is a statement about the pool
    // only for as long as the wait was, and a shortened budget can fall below
    // the spacing between two starts — measured, a pool serving 42 clients
    // through a 200 ms handler with the budget down at 62 ms reported 108 of
    // these, every one of them a wait that ended between two starts on a pool
    // that was working the whole time.
    if wait_at_ceiling && crate::metrics::pool_starts() == starts_on_arrival {
        metrics.admission_wait_wasted();
    }
    Ok(crate::types::ScriptResponse::overloaded())
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
    coding: Option<compression::Coding>,
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
                coding,
                server.compression,
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

            // The request itself is about to be handed to the executor, and
            // the queue deadline has to be answerable from this side after
            // that: the pool reads it at pickup, and the case this covers is
            // the one where no pickup happens.
            let deadline_cancel = script_request.cancel_state.clone();

            let queue_start = Instant::now();
            // Paired with the reading taken if this request is ever refused on
            // its deadline: the two together say whether the pool began any
            // work at all while it waited.
            //
            // Taken on arrival rather than on entry to the queue, because the
            // budget it is paired with is stamped on arrival too. A request
            // that spends most of that budget waiting for a slot is queued
            // behind a line that is moving, and a reading taken at the end of
            // that wait would report it as a wait nobody could have served.
            // The gate takes its own reading for the wait that ends there.
            let starts_on_arrival = crate::metrics::pool_starts();
            // Guard, not a matching decrement: it discounts the request from
            // `pending_requests` however this scope ends, including the client
            // vanishing mid-await and taking the whole future with it.
            let pending = server.metrics.request_queued();
            // `Admitting` means the queue was full and the request is waiting
            // for a slot. `Err(())` below is the worker dropping the response
            // channel, never a shed — a shed arrives as an ordinary response.
            //
            // `busy_workers` is deliberately not claimed here. Nothing this
            // scope can observe distinguishes "queued" from "running", and a
            // gauge named for worker threads must not count either the waiting
            // set or the channel backlog; it is derived from the worker
            // registry at scrape time instead.
            let mut rejected = false;
            let settled = match server.executor.execute(script_request) {
                ExecuteResult::Immediate(resp) => Ok(resp),
                ExecuteResult::Rejected(resp) => {
                    rejected = true;
                    Ok(resp)
                }
                ExecuteResult::Deferred(queued) => {
                    await_queued(
                        queued,
                        &deadline_cancel,
                        &server.metrics,
                        starts_on_arrival,
                        &mut rejected,
                    )
                    .await
                }
                ExecuteResult::Admitting(admission) => match admission.await {
                    Ok(queued) => {
                        await_queued(
                            queued,
                            &deadline_cancel,
                            &server.metrics,
                            starts_on_arrival,
                            &mut rejected,
                        )
                        .await
                    }
                    Err(resp) => {
                        rejected = true;
                        Ok(resp)
                    }
                },
            };

            let (mut script_response, exec_data) = match settled {
                Ok(resp) => {
                    drop(pending);
                    let php_exec_us = resp.execution_time_us;
                    // Everything between dispatch and the response, minus the
                    // time PHP spent running: waiting for admission plus
                    // waiting in the queue. The elapsed time on its own is not
                    // a queue wait — it also carries the script's execution
                    // time, so an idle server reported its own PHP latency as
                    // queueing and the metric could not answer the question it
                    // is named for.
                    let queue_wait_us =
                        (queue_start.elapsed().as_micros() as u64).saturating_sub(php_exec_us);
                    // A refused request never queued at all: the fail-fast shed
                    // would contribute a zero and the budget-expired shed the
                    // whole budget, either way describing refusals rather than
                    // queueing. `oxphp_admission_refused_total` counts those instead.
                    // This has to hold for the trace attribute below as well as
                    // the histogram — a span claiming a second of queue wait for
                    // a request that never entered the queue is the same lie,
                    // told where it is harder to cross-check.
                    // Two flags because those answers arrive two ways.
                    // `resp.refused` covers the ones a worker decides: a
                    // request reached past its queue deadline comes back
                    // through the ordinary response channel, so the flag on
                    // the executor side never sees it. `rejected` covers the
                    // one this side decides, where the deadline passes before
                    // any worker takes the request and no response is ever
                    // sent through that channel at all.
                    //
                    // Neither flag means "refused" exactly — a deadline that
                    // passed on a request whose client had already gone is
                    // answered `499` and counted as a cancellation, not as a
                    // refusal. What both mean is the thing this line is
                    // deciding: no worker picked the request up, so there is
                    // no pickup latency to record.
                    let queue_wait_us = (!rejected && !resp.refused).then_some(queue_wait_us);
                    if let Some(us) = queue_wait_us {
                        server.metrics.record_queue_wait(us);
                    }
                    (
                        resp,
                        PhpExecData {
                            queue_wait_us,
                            php_exec_us: Some(php_exec_us),
                            ..PhpExecData::default()
                        },
                    )
                }
                Err(()) => {
                    drop(pending);
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
            };

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
                profile_tree: script_response.profile_tree.take(),
                cancel_reason: Some(script_response.cancel_reason),
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

    /// Builds the pieces `await_queued` needs, with `deadline` measured from
    /// now. The pickup reading is taken here rather than passed in, so a test
    /// that wants it to have moved says so explicitly.
    fn queued_for(
        deadline: Option<std::time::Duration>,
    ) -> (
        tokio::sync::oneshot::Sender<crate::types::ScriptResponse>,
        crate::executor::Queued,
        crate::bridge::cancel::CancellationState,
        crate::metrics::Metrics,
    ) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let queued = crate::executor::Queued {
            rx,
            deadline: deadline.map(|d| std::time::Instant::now() + d),
            // The configured budget unless a test says otherwise: the
            // shortened-budget case has one of its own below.
            wait_at_ceiling: true,
        };
        (
            tx,
            queued,
            crate::bridge::cancel::CancellationState::new(),
            crate::metrics::Metrics::new(),
        )
    }

    /// A worker's answer, distinguishable from the 529 the deadline produces.
    fn ok_response() -> crate::types::ScriptResponse {
        crate::types::ScriptResponse::default()
    }

    /// Fail-fast mode arms no timer, so a worker that takes longer than any
    /// budget would have allowed is still waited for. The delay here outlives
    /// the budget the neighbouring tests refuse on by three times over.
    #[tokio::test(flavor = "current_thread")]
    async fn await_queued_without_a_deadline_waits_for_the_worker() {
        let (tx, queued, cancel, metrics) = queued_for(None);
        let mut rejected = false;
        let handle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            let _ = tx.send(ok_response());
        });

        let resp = await_queued(
            queued,
            &cancel,
            &metrics,
            crate::metrics::pool_starts(),
            &mut rejected,
        )
        .await
        .expect("the worker answered");

        handle.await.unwrap();
        assert_eq!(resp.status, 200);
        assert!(!rejected);
        assert!(metrics
            .to_prometheus()
            .contains("oxphp_admission_refused_total{reason=\"wait_timeout\"} 0"));
    }

    /// A response already on the channel wins over a deadline that has also
    /// come due: `biased` polls it first, and answering work that is done beats
    /// refusing it.
    ///
    /// Repeated, because a `select!` without `biased` chooses between ready
    /// arms at random and one run of it is a coin toss — the property is that
    /// the answer wins *every* time, and only a run of them can say so.
    #[tokio::test(flavor = "current_thread")]
    async fn await_queued_prefers_a_finished_response_to_an_expired_deadline() {
        for attempt in 0..32 {
            // Already in the past when the select runs, so both arms are ready
            // and nothing but the polling order decides between them.
            let (tx, queued, cancel, metrics) = queued_for(None);
            let queued = crate::executor::Queued {
                deadline: Some(std::time::Instant::now() - std::time::Duration::from_secs(1)),
                ..queued
            };
            let mut rejected = false;
            tx.send(ok_response()).unwrap();

            let resp = await_queued(
                queued,
                &cancel,
                &metrics,
                crate::metrics::pool_starts(),
                &mut rejected,
            )
            .await
            .expect("the finished response came back");

            assert_eq!(
                resp.status, 200,
                "attempt {attempt} refused a response that was already in hand"
            );
            assert!(!rejected);
            // The claim is untouched: this side never had to take it, so a
            // worker arriving afterwards is still free to.
            assert!(cancel.claim_from_queue());
        }
    }

    /// The case the fix exists for: nobody picks the request up, so the
    /// deadline is the only thing that can answer it.
    #[tokio::test(flavor = "current_thread")]
    async fn await_queued_refuses_a_request_no_worker_ever_takes() {
        let (tx, queued, cancel, metrics) = queued_for(Some(std::time::Duration::from_millis(50)));
        let mut rejected = false;
        // Bounds the failure rather than the success: a build that arms no
        // timer would otherwise wait on this channel forever, and a test that
        // hangs reports nothing. Dropping the sender well after the deadline
        // turns that into a failed `expect` without touching the path being
        // measured, which has answered and returned long before.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            drop(tx);
        });

        let resp = await_queued(
            queued,
            &cancel,
            &metrics,
            crate::metrics::pool_starts(),
            &mut rejected,
        )
        .await
        .expect("the deadline answered");

        assert_eq!(resp.status, 529);
        assert!(rejected);
        let out = metrics.to_prometheus();
        assert!(out.contains("oxphp_admission_refused_total{reason=\"wait_timeout\"} 1"));
        // Nothing came off the queue while it waited, so the wait never stood a
        // chance — which is the distinction this counter carries.
        assert!(out.contains("oxphp_admission_wait_wasted_total 1"));
    }

    /// Same refusal, but the pool was moving: the request lost a race for a
    /// worker rather than never having one. Only the subset counter separates
    /// the two, so it has to stay still here.
    #[tokio::test(flavor = "current_thread")]
    async fn await_queued_does_not_call_a_lost_race_a_wasted_wait() {
        let (tx, queued, cancel, metrics) = queued_for(Some(std::time::Duration::from_millis(50)));
        let mut rejected = false;
        let moved = crate::metrics::pool_starts().wrapping_sub(1);
        // As above: bounds a build that arms no timer, so it fails instead of
        // hanging. Well past the deadline this one answers on.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            drop(tx);
        });

        let resp = await_queued(queued, &cancel, &metrics, moved, &mut rejected)
            .await
            .expect("the deadline answered");

        assert_eq!(resp.status, 529);
        let out = metrics.to_prometheus();
        assert!(out.contains("oxphp_admission_refused_total{reason=\"wait_timeout\"} 1"));
        assert!(out.contains("oxphp_admission_wait_wasted_total 0"));
    }

    /// The same refusal again, on a wait the server itself had shortened. The
    /// pool may well have started nothing in a window that short, and saying
    /// so would report a working pool as one that picks up nothing at all —
    /// the reading is only meaningful over the window the operator configured.
    #[tokio::test(flavor = "current_thread")]
    async fn await_queued_does_not_judge_a_shortened_wait_against_the_pool() {
        let (tx, queued, cancel, metrics) = queued_for(Some(std::time::Duration::from_millis(50)));
        let queued = crate::executor::Queued {
            wait_at_ceiling: false,
            ..queued
        };
        let mut rejected = false;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            drop(tx);
        });

        let resp = await_queued(
            queued,
            &cancel,
            &metrics,
            // Nothing started while it waited — the ceiling condition is the
            // only thing standing between this and a false report.
            crate::metrics::pool_starts(),
            &mut rejected,
        )
        .await
        .expect("the deadline answered");

        assert_eq!(resp.status, 529);
        assert!(rejected);
        let out = metrics.to_prometheus();
        assert!(out.contains("oxphp_admission_refused_total{reason=\"wait_timeout\"} 1"));
        assert!(out.contains("oxphp_admission_wait_wasted_total 0"));
    }

    /// The budget ran out on a request whose client had already gone. Both
    /// are true of it, and only one of them is worth reporting: the refusal
    /// reaches nobody, while `oxphp_admission_refused_total{reason=
    /// "wait_timeout"}` is documented as the reason to shorten
    /// `QUEUE_WAIT_TIMEOUT_MS` — a remedy for the pool, aimed here at how long
    /// clients were prepared to wait. The cancellation is counted where it
    /// happened (the abort guard), so this side must leave both refusal series
    /// alone and answer 499.
    #[tokio::test(flavor = "current_thread")]
    async fn await_queued_does_not_charge_a_departed_client_with_an_overload_refusal() {
        let (tx, queued, cancel, metrics) = queued_for(Some(std::time::Duration::from_millis(50)));
        let mut rejected = false;
        // What the abort guard leaves behind when hyper drops the request
        // future: the cell is set long before this deadline comes due.
        assert!(cancel.set(CancelReason::ClientAbort));
        // As in the neighbouring refusal tests: bounds a build that arms no
        // timer so it fails rather than hangs.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            drop(tx);
        });

        let resp = await_queued(
            queued,
            &cancel,
            &metrics,
            crate::metrics::pool_starts(),
            &mut rejected,
        )
        .await
        .expect("the deadline answered");

        assert_eq!(
            resp.status, 499,
            "a request answered after its client left is a client abort, not an overload refusal"
        );
        assert!(
            rejected,
            "no worker picked this request up, so its wait is a budget that ran out and not a pickup latency — it belongs out of oxphp_queue_wait_us"
        );
        let out = metrics.to_prometheus();
        assert!(
            out.contains("oxphp_admission_refused_total{reason=\"wait_timeout\"} 0"),
            "a refusal nobody receives was counted as one the pool handed out"
        );
        assert!(
            out.contains("oxphp_admission_wait_wasted_total 0"),
            "the departed client moved the counter that argues for a shorter budget"
        );
    }

    /// A worker that claimed the request a moment before the timer fired owns
    /// it: this side answers nothing and counts nothing, or one refusal would
    /// be served twice and counted twice.
    #[tokio::test(flavor = "current_thread")]
    async fn await_queued_leaves_a_claimed_request_to_the_worker() {
        let (tx, queued, cancel, metrics) = queued_for(Some(std::time::Duration::from_millis(50)));
        let mut rejected = false;
        assert!(
            cancel.claim_from_queue(),
            "the worker takes the claim first"
        );
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            let _ = tx.send(ok_response());
        });

        let resp = await_queued(
            queued,
            &cancel,
            &metrics,
            crate::metrics::pool_starts(),
            &mut rejected,
        )
        .await
        .expect("the worker answered");

        assert_eq!(resp.status, 200);
        assert!(!rejected);
        let out = metrics.to_prometheus();
        assert!(out.contains("oxphp_admission_refused_total{reason=\"wait_timeout\"} 0"));
        assert!(out.contains("oxphp_admission_wait_wasted_total 0"));
    }

    /// The same claim, on a request whose client has also gone. Both things
    /// the 499 branch reads are true here, and it must still not fire: the
    /// worker owns the request and an answer is already on its way, so a
    /// second one written from this side would be a response the caller never
    /// asked for and a wait excluded from `oxphp_queue_wait_us` that a worker
    /// did pick up. Which is why that branch sits after the claim and not
    /// before it.
    #[tokio::test(flavor = "current_thread")]
    async fn await_queued_leaves_a_claimed_departed_request_to_the_worker() {
        let (tx, queued, cancel, metrics) = queued_for(Some(std::time::Duration::from_millis(50)));
        let mut rejected = false;
        assert!(cancel.set(CancelReason::ClientAbort));
        assert!(
            cancel.claim_from_queue(),
            "the worker takes the claim first"
        );
        // What the worker answers is not the point — that it is the worker's
        // answer which comes back is. A 200 is the one thing this side would
        // never write for itself.
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            let _ = tx.send(ok_response());
        });

        let resp = await_queued(
            queued,
            &cancel,
            &metrics,
            crate::metrics::pool_starts(),
            &mut rejected,
        )
        .await
        .expect("the worker answered");

        assert_eq!(
            resp.status, 200,
            "this side answered a request a worker had already claimed"
        );
        assert!(
            !rejected,
            "a worker did pick this request up, so its wait is a pickup latency and belongs in oxphp_queue_wait_us"
        );
        let out = metrics.to_prometheus();
        assert!(out.contains("oxphp_admission_refused_total{reason=\"wait_timeout\"} 0"));
    }

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

    /// Disarming must release the guard's `Arc`, not just neutralise `Drop`.
    /// A `mem::forget`-style disarm keeps the strong count above zero forever,
    /// leaking one 128-byte `Arc<CancellationState>` heap block per successful
    /// request — invisible to functional tests, ~11 GB/day at 1000 rps.
    #[test]
    fn test_disarm_releases_cancel_state() {
        let state = std::sync::Arc::new(CancellationState::new());
        let metrics = Metrics::new();
        let guard = ClientAbortGuard::new(std::sync::Arc::clone(&state), &metrics);
        assert_eq!(std::sync::Arc::strong_count(&state), 2);

        guard.disarm(Some(CancelReason::None as u8));

        assert_eq!(
            std::sync::Arc::strong_count(&state),
            1,
            "disarm must drop the guard's Arc, not leak it"
        );
        assert!(state.is_done(), "disarm must mark the state done");
        assert_eq!(
            state.get(),
            CancelReason::None,
            "disarm must not report a cancellation"
        );
        assert_eq!(
            metrics
                .request_cancelled_client_abort
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "a request that finished normally was counted as cancelled"
        );
    }

    /// The counterpart: dropping without disarming must still fire the abort,
    /// so a leak fix cannot degenerate into a guard that never cancels.
    /// A failure here can be collateral — `cancel_request` takes the worker
    /// registry's one poison-intolerant lock.
    #[test]
    fn test_drop_without_disarm_cancels() {
        let state = std::sync::Arc::new(CancellationState::new());
        let metrics = Metrics::new();
        drop(ClientAbortGuard::new(
            std::sync::Arc::clone(&state),
            &metrics,
        ));

        assert_eq!(state.get(), CancelReason::ClientAbort);
        assert_eq!(std::sync::Arc::strong_count(&state), 1);
        assert_eq!(
            metrics
                .request_cancelled_client_abort
                .load(std::sync::atomic::Ordering::Relaxed),
            1,
            "the guard cancelled the request without counting it"
        );
    }

    /// A worker that cancels a request itself — its own timeout, a drain —
    /// takes the disarmed path, because the dispatch future does return. The
    /// guard is still the counting site, so it has to report the reason the
    /// worker stored rather than the one it would have written itself.
    #[test]
    fn test_disarm_counts_the_reason_the_worker_set() {
        let state = std::sync::Arc::new(CancellationState::new());
        let metrics = Metrics::new();
        let guard = ClientAbortGuard::new(std::sync::Arc::clone(&state), &metrics);
        // The cell and the response agree, which is the ordinary case: the
        // worker read this cell to build that response.
        assert!(state.set(CancelReason::Timeout));

        guard.disarm(Some(CancelReason::Timeout as u8));

        assert_eq!(
            metrics
                .request_cancelled_timeout
                .load(std::sync::atomic::Ordering::Relaxed),
            1,
            "a worker-side timeout stopped being counted"
        );
        assert_eq!(
            metrics
                .request_cancelled_client_abort
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "the guard overwrote the worker's reason with its own"
        );
    }

    /// The two disagree in exactly one window, and the response wins it.
    ///
    /// A worker publishes its response and only then unregisters, so a drain
    /// sweep landing between the two reaches a request that has already been
    /// answered and fills its cell. Counting the cell at that point files a
    /// request the client received `200` for under `reason="shutdown"` — a
    /// false reading in the one counter this whole path exists to make
    /// worth trusting. The window is narrow; the counter is not sampled.
    #[test]
    fn a_cancellation_arriving_after_the_answer_is_not_counted() {
        let state = std::sync::Arc::new(CancellationState::new());
        let metrics = Metrics::new();
        let guard = ClientAbortGuard::new(std::sync::Arc::clone(&state), &metrics);

        // The drain, reaching a request whose 200 is already on its way out.
        assert!(state.set(CancelReason::Shutdown));
        guard.disarm(Some(CancelReason::None as u8));

        assert_eq!(
            metrics
                .request_cancelled_shutdown
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "a request answered 200 was counted as cancelled by the drain"
        );
    }

    /// And the fallback still reads the cell, for the answers that never
    /// arrive: a worker channel dropped out from under a request leaves the
    /// cell as the only account of what happened to it.
    #[test]
    fn an_unanswered_request_is_still_counted_from_the_cell() {
        let state = std::sync::Arc::new(CancellationState::new());
        let metrics = Metrics::new();
        let guard = ClientAbortGuard::new(std::sync::Arc::clone(&state), &metrics);

        assert!(state.set(CancelReason::Shutdown));
        guard.disarm(None);

        assert_eq!(
            metrics
                .request_cancelled_shutdown
                .load(std::sync::atomic::Ordering::Relaxed),
            1,
            "a request the drain cancelled and nobody answered went uncounted"
        );
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
