use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{header, Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Limited};
use hyper::body::Body as _;
use hyper::body::Incoming;

use crate::events::{RequestComplete, RequestReceived, ResponseBuilding};
use crate::executor::ExecuteResult;
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

/// Returns true if the method is QUERY (draft-ietf-httpbis-safe-method-w-body).
fn is_query_method(method: &Method) -> bool {
    method.as_str() == "QUERY"
}

/// Look up a key in the metadata vector, returning empty string if not found.
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

/// Handle a single HTTP request with event-driven pipeline.
pub async fn handle_request(
    req: Request<Incoming>,
    server: &Server,
    remote_addr: SocketAddr,
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
    // Handlers: RequestIdGenerator (-100), RateLimitHandler (-50), MetricsRequestHandler (0)
    let mut received_event = RequestReceived {
        parts,
        remote_addr,
        request_id: String::new(),
        early_response: None,
        metadata: Vec::new(),
    };
    server.dispatcher.dispatch(&mut received_event);

    // Take ownership — no clone
    let request_id = std::mem::take(&mut received_event.request_id);
    let mut metadata = std::mem::take(&mut received_event.metadata);

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
        };
        server.dispatcher.dispatch(&mut complete_event);

        return Ok(early_resp);
    }

    let mut parts = received_event.parts;
    // Clone method (cheap enum copy) and path before parts are consumed
    let method = parts.method.clone();
    let path_str = parts.uri.path().to_string();
    crate::plugin::cookies::strip_plugin_cookies(&mut parts);

    // Apply request timeout if configured
    let result = if server.request_timeout > Duration::ZERO {
        match tokio::time::timeout(
            server.request_timeout,
            dispatch_request(parts, body, server, remote_addr, &request_id, &mut metadata),
        )
        .await
        {
            Ok(inner_result) => inner_result,
            Err(_) => {
                tracing::warn!(
                    request_id = %request_id,
                    path = %path_str,
                    timeout_secs = server.request_timeout.as_secs(),
                    "Request timeout"
                );
                Ok((
                    Response::builder()
                        .status(StatusCode::REQUEST_TIMEOUT)
                        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                        .body(full_body(Bytes::from_static(b"408 Request Timeout")))
                        .unwrap(),
                    0usize,
                ))
            }
        }
    } else {
        dispatch_request(parts, body, server, remote_addr, &request_id, &mut metadata).await
    };

    let (response, request_body_size) = match result {
        Ok((resp, body_size)) => (resp, body_size),
        Err(e) => {
            tracing::error!(error = %e, path = %path_str, request_id = %request_id, "Internal server error");
            (
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                    .body(full_body(Bytes::from_static(b"500 Internal Server Error")))
                    .unwrap(),
                0,
            )
        }
    };

    // ── ResponseBuilding event ──
    // Handlers: TraceContextResponseHandler (-95), ErrorPagesHandler (60), ServerHeaderHandler (100)
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
    };
    server.dispatcher.dispatch(&mut complete_event);

    Ok(response)
}

/// Returns (response, request_body_size).
async fn dispatch_request(
    parts: http::request::Parts,
    body: Incoming,
    server: &Server,
    remote_addr: SocketAddr,
    request_id: &str,
    metadata: &mut Vec<(String, String)>,
) -> Result<(Response<ResponseBody>, usize), crate::types::BoxError> {
    let uri_path = parts.uri.path();
    let route_result = server
        .route_config
        .resolve_request(uri_path, &server.file_cache)
        .await;

    let mut request_body_size = 0usize;

    let response = match route_result {
        RouteResult::Serve(file_path) => {
            // Read-only cache check: read lock, no LRU update, no stat() syscall
            let cache_key = file_path.to_string_lossy();
            let was_cached = server.file_cache.content_cached(&cache_key);

            let response = static_file::serve(
                &file_path,
                &server.file_cache,
                server.route_config.canonical_root(),
                &parts.headers,
                server.static_cache_control.as_deref(),
            )
            .await?;

            if was_cached {
                server.metrics.static_cache_hit();
            } else {
                server.metrics.static_cache_miss();
            }

            response
        }
        RouteResult::Execute(script_path) => {
            let is_query = is_query_method(&parts.method);

            // QUERY requires Content-Type per draft-ietf-httpbis-safe-method-w-body §4.2
            if is_query && !parts.headers.contains_key(http::header::CONTENT_TYPE) {
                return Ok((
                    Response::builder()
                        .status(StatusCode::UNSUPPORTED_MEDIA_TYPE)
                        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                        .body(full_body(Bytes::from_static(
                            b"415 Unsupported Media Type: QUERY requires Content-Type",
                        )))?,
                    0,
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
                timeout_us: server.request_timeout.as_micros() as u64,
                trace_id: metadata_get(metadata, "trace_id").to_string(),
                span_id: metadata_get(metadata, "span_id").to_string(),
                parent_span_id: metadata_get(metadata, "parent_span_id").to_string(),
                is_tls: server.is_tls(),
                version: parts.version,
            };

            let queue_start = Instant::now();
            server.metrics.request_queued();
            let execute_result = server.executor.execute(script_request);

            let script_response = match execute_result {
                ExecuteResult::Immediate(resp) => {
                    server.metrics.request_dequeued();
                    let queue_wait_us = queue_start.elapsed().as_micros();
                    server.metrics.record_queue_wait(queue_wait_us as u64);
                    metadata.push(("oxphp.queue_wait_us".into(), queue_wait_us.to_string()));
                    metadata.push((
                        "oxphp.php_exec_us".into(),
                        resp.execution_time_us.to_string(),
                    ));
                    resp
                }
                ExecuteResult::Deferred(rx) => match rx.await {
                    Ok(resp) => {
                        server.metrics.request_dequeued();
                        let queue_wait_us = queue_start.elapsed().as_micros();
                        server.metrics.record_queue_wait(queue_wait_us as u64);
                        metadata.push(("oxphp.queue_wait_us".into(), queue_wait_us.to_string()));
                        metadata.push((
                            "oxphp.php_exec_us".into(),
                            resp.execution_time_us.to_string(),
                        ));
                        resp
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
                        ));
                    }
                },
            };

            let mut builder = Response::builder().status(script_response.status);
            for (name, value) in &script_response.headers {
                builder = builder.header(name, value);
            }
            if let Some(rx) = script_response.stream_rx {
                // Streaming response: body arrives as chunks via mpsc channel
                builder.body(stream_body(script_response.body, rx)).unwrap()
            } else {
                builder.body(full_body(script_response.body)).unwrap()
            }
        }
        RouteResult::NotFound => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(full_body(Bytes::from_static(b"404 Not Found")))?,
    };

    Ok((response, request_body_size))
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
