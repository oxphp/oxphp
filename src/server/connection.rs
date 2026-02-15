use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{Method, Request, Response, StatusCode};
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;

use crate::events::{RequestComplete, RequestReceived, ResponseBuilding};
use crate::server::compression;
use crate::server::response::static_file;
use crate::server::routing::RouteResult;
use crate::server::Server;
use crate::types::{full_body, ResponseBody, ScriptRequest};

/// Maximum POST body size (10 MB). Requests exceeding this are rejected with 413.
const MAX_POST_BODY: usize = 10 * 1024 * 1024;

/// Handle a single HTTP request with event-driven pipeline.
pub async fn handle_request(
    req: Request<Incoming>,
    server: &Server,
    remote_addr: SocketAddr,
) -> Result<Response<ResponseBody>, Infallible> {
    let start = Instant::now();
    let (parts, body) = req.into_parts();

    // Check brotli support before parts are consumed by the pipeline (no alloc)
    let supports_brotli = server.compression_enabled
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

    // Check for early response (e.g., 429 from rate limiter)
    if let Some(early_resp) = received_event.early_response {
        let status = early_resp.status().as_u16();
        let elapsed = start.elapsed();

        // Dispatch RequestComplete for the early response
        let mut complete_event = RequestComplete {
            request_id,
            method: received_event.parts.method.clone(),
            path: received_event.parts.uri.path().to_string(),
            status,
            duration: elapsed,
            remote_addr,
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
            dispatch_request(parts, body, server, remote_addr, &request_id),
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
                Ok(Response::builder()
                    .status(StatusCode::GATEWAY_TIMEOUT)
                    .body(full_body(Bytes::from_static(b"504 Gateway Timeout")))
                    .unwrap())
            }
        }
    } else {
        dispatch_request(parts, body, server, remote_addr, &request_id).await
    };

    let response = match result {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!(error = %e, path = %path_str, request_id = %request_id, "Internal server error");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(full_body(Bytes::from_static(b"500 Internal Server Error")))
                .unwrap()
        }
    };

    // ── ResponseBuilding event ──
    // Handlers: ErrorPagesHandler (60), ServerHeaderHandler (100)
    let mut building_event = ResponseBuilding {
        request_id, // move in (handlers only read &str)
        response,
    };
    server.dispatcher.dispatch(&mut building_event);
    let response = building_event.response;
    let request_id = building_event.request_id; // move back out

    // ── Brotli compression (after error pages, before metrics/logging) ──
    let response = if supports_brotli {
        compression::maybe_compress(response).await
    } else {
        response
    };

    // ── RequestComplete event ──
    // Handlers: MetricsResponseHandler (0), AccessLogHandler (100)
    let status = response.status().as_u16();
    let elapsed = start.elapsed();
    let mut complete_event = RequestComplete {
        request_id, // move — no clone
        method,
        path: path_str,
        status,
        duration: elapsed,
        remote_addr,
    };
    server.dispatcher.dispatch(&mut complete_event);

    Ok(response)
}

async fn dispatch_request(
    parts: http::request::Parts,
    body: Incoming,
    server: &Server,
    remote_addr: SocketAddr,
    request_id: &str,
) -> Result<Response<ResponseBody>, crate::types::BoxError> {
    let uri_path = parts.uri.path();
    let route_result = server
        .route_config
        .resolve_request(uri_path, &server.file_cache)
        .await;

    let response = match route_result {
        RouteResult::Serve(file_path) => {
            static_file::serve(
                &file_path,
                &server.file_cache,
                server.route_config.canonical_root(),
            )
            .await?
        }
        RouteResult::Execute(script_path) => {
            // Skip body collection for methods that don't carry a payload
            let body_bytes = if matches!(parts.method, Method::POST | Method::PUT | Method::PATCH) {
                let limited = Limited::new(body, MAX_POST_BODY);
                match BodyExt::collect(limited).await {
                    Ok(collected) => collected.to_bytes(),
                    Err(e) => {
                        if e.downcast_ref::<http_body_util::LengthLimitError>()
                            .is_some()
                        {
                            return Ok(Response::builder()
                                .status(StatusCode::PAYLOAD_TOO_LARGE)
                                .body(full_body(Bytes::from_static(
                                b"413 Payload Too Large",
                            )))?);
                        }
                        return Err(e);
                    }
                }
            } else {
                Bytes::new()
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
                document_root: server.route_config.document_root_arc(),
            };

            server.metrics.request_queued();
            let response_rx = server.executor.execute(script_request);

            match response_rx.await {
                Ok(script_response) => {
                    server.metrics.request_dequeued();

                    let mut builder = Response::builder().status(script_response.status);

                    for (name, value) in &script_response.headers {
                        builder = builder.header(name, value);
                    }

                    builder.body(full_body(script_response.body)).unwrap()
                }
                Err(_) => {
                    server.metrics.request_dropped();
                    Response::builder()
                        .status(StatusCode::INTERNAL_SERVER_ERROR)
                        .body(full_body(Bytes::from_static(b"500 PHP Worker Error")))
                        .unwrap()
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

    use crate::events::EventHandler;
    use crate::handlers::request_id::RequestIdGenerator;

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
        assert_eq!(event.request_id.len(), 16);
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
