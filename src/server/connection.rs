use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use http::request::Parts;
use http::{HeaderValue, Request, Response, StatusCode};
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;

use crate::executor::ScriptExecutor;
use crate::metrics::Metrics;
use crate::server::error_pages::ErrorPages;
use crate::server::rate_limit::RateLimiter;
use crate::server::response::static_file::{self, FileCache};
use crate::server::routing::{RouteConfig, RouteResult};
use crate::types::{full_body, ResponseBody, ScriptRequest};

/// Maximum POST body size (10 MB). Requests exceeding this are rejected with 413.
const MAX_POST_BODY: usize = 10 * 1024 * 1024;

/// Pre-computed `Server` header value — avoids allocation per response.
static SERVER_HEADER_VALUE: LazyLock<HeaderValue> =
    LazyLock::new(|| HeaderValue::from_static(concat!("OxPHP/", env!("CARGO_PKG_VERSION"))));

/// Atomic counter for request ID generation.
static REQUEST_COUNTER: AtomicU32 = AtomicU32::new(0);

/// Generate a request ID: `{timestamp_hex:08x}{counter:08x}` (16 hex chars).
/// Uses full u32 counter range (~4 billion IDs before wrap) for collision resistance.
fn generate_request_id() -> String {
    let ts = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as u32;
    let counter = REQUEST_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{ts:08x}{counter:08x}")
}

/// Shared per-request context passed through the request pipeline.
pub struct RequestContext<'a> {
    pub route_config: &'a RouteConfig,
    pub file_cache: &'a Arc<FileCache>,
    pub executor: &'a Arc<dyn ScriptExecutor>,
    pub metrics: &'a Metrics,
    pub rate_limiter: Option<&'a RateLimiter>,
    pub error_pages: Option<&'a ErrorPages>,
    pub request_timeout: Duration,
}

/// Handle a single HTTP request with metrics, rate limiting, timeouts, and error pages.
#[allow(clippy::too_many_arguments)]
pub async fn handle_request(
    req: Request<Incoming>,
    route_config: &RouteConfig,
    file_cache: &Arc<FileCache>,
    executor: &Arc<dyn ScriptExecutor>,
    remote_addr: SocketAddr,
    metrics: &Metrics,
    rate_limiter: Option<&RateLimiter>,
    error_pages: Option<&ErrorPages>,
    request_timeout: Duration,
) -> Result<Response<ResponseBody>, Infallible> {
    let ctx = RequestContext {
        route_config,
        file_cache,
        executor,
        metrics,
        rate_limiter,
        error_pages,
        request_timeout,
    };
    handle_request_with_ctx(req, &ctx, remote_addr).await
}

async fn handle_request_with_ctx(
    req: Request<Incoming>,
    ctx: &RequestContext<'_>,
    remote_addr: SocketAddr,
) -> Result<Response<ResponseBody>, Infallible> {
    let start = Instant::now();
    let (parts, body) = req.into_parts();

    // Record request metric
    ctx.metrics.record_request(&parts.method);

    // Determine request ID: honor incoming X-Request-ID or generate
    let request_id = parts
        .headers
        .get("x-request-id")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .unwrap_or_else(generate_request_id);

    let method_str = parts.method.to_string();
    let path_str = parts.uri.path().to_string();

    // Rate limiting check
    if let Some(limiter) = ctx.rate_limiter {
        if let Some(resp) = limiter.check_rate_limited(remote_addr.ip(), &request_id) {
            let status = resp.status().as_u16();
            let elapsed = start.elapsed();
            ctx.metrics.record_response(status, elapsed);
            tracing::info!(
                target: "access_log",
                request_id = %request_id,
                method = %method_str,
                path = %path_str,
                status = status,
                duration_us = elapsed.as_micros() as u64,
                remote_addr = %remote_addr,
                "request completed"
            );
            return Ok(resp);
        }
    }

    let uri = parts.uri.clone();

    // Apply request timeout if configured
    let result = if ctx.request_timeout > Duration::ZERO {
        match tokio::time::timeout(
            ctx.request_timeout,
            dispatch_request(parts, body, ctx, remote_addr, &request_id),
        )
        .await
        {
            Ok(inner_result) => inner_result,
            Err(_) => {
                tracing::warn!(
                    request_id = %request_id,
                    uri = %uri,
                    timeout_secs = ctx.request_timeout.as_secs(),
                    "Request timeout"
                );
                Ok(Response::builder()
                    .status(StatusCode::GATEWAY_TIMEOUT)
                    .body(full_body(Bytes::from_static(b"504 Gateway Timeout")))
                    .unwrap())
            }
        }
    } else {
        dispatch_request(parts, body, ctx, remote_addr, &request_id).await
    };

    let mut response = match result {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!(error = %e, uri = %uri, request_id = %request_id, "Internal server error");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(full_body(Bytes::from_static(b"500 Internal Server Error")))
                .unwrap()
        }
    };

    // Apply custom error page if applicable
    let status = response.status().as_u16();
    if status >= 400 {
        if let Some(pages) = ctx.error_pages {
            if let Some(page_bytes) = pages.get(status) {
                response = Response::builder()
                    .status(
                        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                    )
                    .header(http::header::CONTENT_TYPE, "text/html; charset=utf-8")
                    .header(http::header::CONTENT_LENGTH, page_bytes.len())
                    .body(full_body(page_bytes))
                    .unwrap();
            }
        }
    }

    // Add standard headers
    response
        .headers_mut()
        .insert(http::header::SERVER, SERVER_HEADER_VALUE.clone());

    if let Ok(hv) = HeaderValue::from_str(&request_id) {
        response.headers_mut().insert("x-request-id", hv);
    }

    // Record response metric
    let elapsed = start.elapsed();
    ctx.metrics.record_response(status, elapsed);

    // Access log
    tracing::info!(
        target: "access_log",
        request_id = %request_id,
        method = %method_str,
        path = %path_str,
        status = status,
        duration_us = elapsed.as_micros() as u64,
        remote_addr = %remote_addr,
        "request completed"
    );

    Ok(response)
}

async fn dispatch_request(
    parts: Parts,
    body: Incoming,
    ctx: &RequestContext<'_>,
    remote_addr: SocketAddr,
    request_id: &str,
) -> Result<Response<ResponseBody>, crate::types::BoxError> {
    let uri_path = parts.uri.path();
    let route_result = ctx
        .route_config
        .resolve_request(uri_path, ctx.file_cache)
        .await;

    let response = match route_result {
        RouteResult::Serve(file_path) => {
            static_file::serve(
                &file_path,
                ctx.file_cache,
                ctx.route_config.canonical_root(),
            )
            .await?
        }
        RouteResult::Execute(script_path) => {
            let limited = Limited::new(body, MAX_POST_BODY);
            let body_bytes = match BodyExt::collect(limited).await {
                Ok(collected) => collected.to_bytes(),
                Err(e) => {
                    if e.downcast_ref::<http_body_util::LengthLimitError>()
                        .is_some()
                    {
                        return Ok(Response::builder()
                            .status(StatusCode::PAYLOAD_TOO_LARGE)
                            .body(full_body(Bytes::from_static(b"413 Payload Too Large")))?);
                    }
                    return Err(e);
                }
            };

            let query_string = parts.uri.query().unwrap_or("").to_string();

            let script_request = ScriptRequest {
                request_id: request_id.to_string(),
                script_path,
                method: parts.method,
                uri: parts.uri,
                query_string,
                headers: parts.headers,
                body: body_bytes,
                remote_addr,
                document_root: ctx.route_config.document_root_arc(),
            };

            ctx.metrics.request_queued();
            let response_rx = ctx.executor.execute(script_request);

            match response_rx.await {
                Ok(script_response) => {
                    ctx.metrics.request_dequeued();

                    let mut builder = Response::builder().status(script_response.status);

                    for (name, value) in &script_response.headers {
                        builder = builder.header(name, value);
                    }

                    builder.body(full_body(script_response.body))?
                }
                Err(_) => {
                    ctx.metrics.request_dropped();
                    Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(full_body(Bytes::from_static(b"500 PHP Worker Error")))?
                }
            }
        }
        RouteResult::NotFound => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(full_body(Bytes::from_static(b"404 Not Found")))?,
    };

    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_id_generation() {
        let id = generate_request_id();
        assert_eq!(id.len(), 16);
        // Should be valid hex
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn test_request_id_uniqueness() {
        let ids: Vec<String> = (0..100).map(|_| generate_request_id()).collect();
        let mut deduped = ids.clone();
        deduped.sort();
        deduped.dedup();
        assert_eq!(ids.len(), deduped.len());
    }
}
