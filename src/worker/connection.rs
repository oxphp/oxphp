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
use crate::types::{full_body, ResponseBody, ScriptRequest};
use crate::worker::{ExecutorMode, SharedState};

/// Maximum POST body size (10 MB). Requests exceeding this are rejected with 413.
const MAX_POST_BODY: usize = 10 * 1024 * 1024;

/// Handle a single HTTP request with event-driven pipeline.
///
/// Called by hyper's `service_fn` for each request on the connection.
/// Stub: returns static response inline. SAPI: calls `execute_request()` directly.
pub async fn handle_request(
    req: Request<Incoming>,
    state: &SharedState,
    remote_addr: SocketAddr,
) -> Result<Response<ResponseBody>, Infallible> {
    let start = Instant::now();
    let (parts, body) = req.into_parts();

    // Check brotli support before parts are consumed by the pipeline (no alloc)
    let supports_brotli = state.compression_enabled
        && parts
            .headers
            .get(http::header::ACCEPT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .is_some_and(compression::accepts_brotli);

    // ── RequestReceived event ──
    let mut received_event = RequestReceived {
        parts,
        remote_addr,
        request_id: String::new(),
        early_response: None,
        metadata: Vec::new(),
    };
    state.dispatcher.dispatch(&mut received_event);

    let request_id = std::mem::take(&mut received_event.request_id);

    // Check for early response (e.g., 429 from rate limiter)
    if let Some(early_resp) = received_event.early_response {
        let status = early_resp.status().as_u16();
        let elapsed = start.elapsed();

        let mut complete_event = RequestComplete {
            request_id,
            method: received_event.parts.method.clone(),
            path: received_event.parts.uri.path().to_string(),
            status,
            duration: elapsed,
            remote_addr,
        };
        state.dispatcher.dispatch(&mut complete_event);

        return Ok(early_resp);
    }

    let mut parts = received_event.parts;
    let method = parts.method.clone();
    let path_str = parts.uri.path().to_string();
    crate::plugin::cookies::strip_plugin_cookies(&mut parts);

    // Apply request timeout if configured
    let result = if state.request_timeout > Duration::ZERO {
        match tokio::time::timeout(
            state.request_timeout,
            dispatch_request(parts, body, state, remote_addr, &request_id),
        )
        .await
        {
            Ok(inner_result) => inner_result,
            Err(_) => {
                tracing::warn!(
                    request_id = %request_id,
                    path = %path_str,
                    timeout_secs = state.request_timeout.as_secs(),
                    "Request timeout"
                );
                Ok(Response::builder()
                    .status(StatusCode::GATEWAY_TIMEOUT)
                    .body(full_body(Bytes::from_static(b"504 Gateway Timeout")))
                    .unwrap())
            }
        }
    } else {
        dispatch_request(parts, body, state, remote_addr, &request_id).await
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
    let mut building_event = ResponseBuilding {
        request_id,
        response,
    };
    state.dispatcher.dispatch(&mut building_event);
    let response = building_event.response;
    let request_id = building_event.request_id;

    // ── Brotli compression ──
    let response = if supports_brotli {
        compression::maybe_compress(response).await
    } else {
        response
    };

    // ── RequestComplete event ──
    let status = response.status().as_u16();
    let elapsed = start.elapsed();
    let mut complete_event = RequestComplete {
        request_id,
        method,
        path: path_str,
        status,
        duration: elapsed,
        remote_addr,
    };
    state.dispatcher.dispatch(&mut complete_event);

    Ok(response)
}

async fn dispatch_request(
    parts: http::request::Parts,
    body: Incoming,
    state: &SharedState,
    remote_addr: SocketAddr,
    request_id: &str,
) -> Result<Response<ResponseBody>, crate::types::BoxError> {
    let uri_path = parts.uri.path();
    let route_result = state
        .route_config
        .resolve_request(uri_path, &state.file_cache)
        .await;

    let response = match route_result {
        RouteResult::Serve(file_path) => {
            static_file::serve(
                &file_path,
                &state.file_cache,
                state.route_config.canonical_root(),
            )
            .await?
        }
        RouteResult::Execute(script_path) => {
            // For stub mode, skip body collection and ScriptRequest construction entirely.
            // This avoids allocating request_id, query_string, headers move, etc.
            if state.mode == ExecutorMode::Stub {
                state.metrics.request_queued();
                state.metrics.request_dequeued();
                return Ok(Response::builder()
                    .status(200)
                    .header("content-type", "text/plain")
                    .body(full_body(Bytes::from_static(b"OK")))
                    .unwrap());
            }

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
                document_root: state.route_config.document_root_arc(),
            };

            state.metrics.request_queued();

            // SAPI: execute PHP directly on this worker thread (no channel).
            // Blocks the single-threaded Tokio runtime — intentional.
            #[cfg(feature = "php")]
            let script_response = {
                let resp = crate::executor::sapi::execute_request(&script_request);
                state.metrics.request_dequeued();
                resp
            };
            #[cfg(not(feature = "php"))]
            {
                let _ = script_request;
                state.metrics.request_dropped();
                return Ok(Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(full_body(Bytes::from_static(
                        b"500 SAPI executor requires php feature",
                    )))
                    .unwrap());
            }

            #[cfg(feature = "php")]
            {
                let mut builder = Response::builder().status(script_response.status);
                for (name, value) in &script_response.headers {
                    builder = builder.header(name, value);
                }
                builder.body(full_body(script_response.body)).unwrap()
            }
        }
        RouteResult::NotFound => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(full_body(Bytes::from_static(b"404 Not Found")))?,
    };

    Ok(response)
}
