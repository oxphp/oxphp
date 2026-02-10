use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;

use crate::events::{EventDispatcher, RequestComplete, RequestReceived, ResponseBuilding};
use crate::executor::ScriptExecutor;
use crate::metrics::Metrics;
use crate::server::response::static_file::{self, FileCache};
use crate::server::routing::{RouteConfig, RouteResult};
use crate::types::{full_body, ResponseBody, ScriptRequest};

/// Maximum POST body size (10 MB). Requests exceeding this are rejected with 413.
const MAX_POST_BODY: usize = 10 * 1024 * 1024;

/// Shared per-request context passed through the request pipeline.
pub struct RequestContext<'a> {
    pub route_config: &'a RouteConfig,
    pub file_cache: &'a Arc<FileCache>,
    pub executor: &'a Arc<dyn ScriptExecutor>,
    pub metrics: &'a Metrics,
    pub dispatcher: &'a EventDispatcher,
    pub request_timeout: Duration,
}

/// Handle a single HTTP request with event-driven pipeline.
#[allow(clippy::too_many_arguments)]
pub async fn handle_request(
    req: Request<Incoming>,
    route_config: &RouteConfig,
    file_cache: &Arc<FileCache>,
    executor: &Arc<dyn ScriptExecutor>,
    remote_addr: SocketAddr,
    metrics: &Metrics,
    dispatcher: &EventDispatcher,
    request_timeout: Duration,
) -> Result<Response<ResponseBody>, Infallible> {
    let ctx = RequestContext {
        route_config,
        file_cache,
        executor,
        metrics,
        dispatcher,
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

    // Cache method/path strings once — moved (not cloned) through the pipeline
    let method_str = parts.method.to_string();
    let path_str = parts.uri.path().to_string();

    // ── RequestReceived event ──
    // Handlers: RequestIdGenerator (-100), RateLimitHandler (-50), MetricsRequestHandler (0)
    let mut received_event = RequestReceived {
        parts,
        remote_addr,
        request_id: String::new(),
        early_response: None,
    };
    ctx.dispatcher.dispatch(&mut received_event);

    // Take ownership — no clone
    let request_id = std::mem::take(&mut received_event.request_id);

    // Check for early response (e.g., 429 from rate limiter)
    if let Some(early_resp) = received_event.early_response {
        let status = early_resp.status().as_u16();
        let elapsed = start.elapsed();

        // Dispatch RequestComplete for the early response
        let mut complete_event = RequestComplete {
            request_id,
            method: method_str,
            path: path_str,
            status,
            duration: elapsed,
            remote_addr,
        };
        ctx.dispatcher.dispatch(&mut complete_event);

        return Ok(early_resp);
    }

    let parts = received_event.parts;
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

    let response = match result {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!(error = %e, uri = %uri, request_id = %request_id, "Internal server error");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(full_body(Bytes::from_static(b"500 Internal Server Error")))
                .unwrap()
        }
    };

    // ── ResponseBuilding event ──
    // Handlers: ErrorPagesHandler (60), ServerHeaderHandler (100)
    let mut building_event = ResponseBuilding {
        request_id: request_id.clone(), // 1 clone (needed: request_id reused in RequestComplete)
        response,
    };
    ctx.dispatcher.dispatch(&mut building_event);
    let response = building_event.response;

    // ── RequestComplete event ──
    // Handlers: MetricsResponseHandler (0), AccessLogHandler (100)
    let status = response.status().as_u16();
    let elapsed = start.elapsed();
    let mut complete_event = RequestComplete {
        request_id, // move — no clone
        method: method_str,
        path: path_str,
        status,
        duration: elapsed,
        remote_addr,
    };
    ctx.dispatcher.dispatch(&mut complete_event);

    Ok(response)
}

async fn dispatch_request(
    parts: http::request::Parts,
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

                    builder.body(full_body(script_response.body)).unwrap()
                }
                Err(_) => {
                    ctx.metrics.request_dropped();
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
