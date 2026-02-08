use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, LazyLock};

use bytes::Bytes;
use http::request::Parts;
use http::{HeaderValue, Request, Response, StatusCode};
use http_body_util::{BodyExt, Limited};
use hyper::body::Incoming;

use crate::executor::ScriptExecutor;
use crate::server::response::static_file::{self, FileCache};
use crate::server::routing::{RouteConfig, RouteResult};
use crate::types::{full_body, ResponseBody, ScriptRequest};

/// Maximum POST body size (10 MB). Requests exceeding this are rejected with 413.
const MAX_POST_BODY: usize = 10 * 1024 * 1024;

/// Pre-computed `Server` header value — avoids allocation per response.
static SERVER_HEADER_VALUE: LazyLock<HeaderValue> =
    LazyLock::new(|| HeaderValue::from_static(concat!("OxPHP/", env!("CARGO_PKG_VERSION"))));

/// Handle a single HTTP request: resolve route, serve static file, execute PHP, or return 404.
/// Returns `Infallible` error type — errors are converted to HTTP 500 responses.
pub async fn handle_request(
    req: Request<Incoming>,
    route_config: &RouteConfig,
    file_cache: &Arc<FileCache>,
    executor: &Arc<dyn ScriptExecutor>,
    remote_addr: SocketAddr,
) -> Result<Response<ResponseBody>, Infallible> {
    let (parts, body) = req.into_parts();

    tracing::debug!(
        method = %parts.method,
        uri = %parts.uri,
        remote_addr = %remote_addr,
        "Handling request"
    );

    let uri = parts.uri.clone();

    let mut response =
        match handle_request_inner(parts, body, route_config, file_cache, executor, remote_addr)
            .await
        {
            Ok(resp) => resp,
            Err(e) => {
                tracing::error!(error = %e, uri = %uri, "Internal server error");
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(full_body(Bytes::from_static(b"500 Internal Server Error")))
                    .unwrap()
            }
        };

    response
        .headers_mut()
        .insert(http::header::SERVER, SERVER_HEADER_VALUE.clone());

    Ok(response)
}

async fn handle_request_inner(
    parts: Parts,
    body: Incoming,
    route_config: &RouteConfig,
    file_cache: &Arc<FileCache>,
    executor: &Arc<dyn ScriptExecutor>,
    remote_addr: SocketAddr,
) -> Result<Response<ResponseBody>, crate::types::BoxError> {
    let uri_path = parts.uri.path();
    let route_result = route_config.resolve_request(uri_path, file_cache).await;

    let response = match route_result {
        RouteResult::Serve(file_path) => {
            static_file::serve(&file_path, file_cache, route_config.canonical_root()).await?
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
                request_id: format!("{:x}", rand_id()),
                script_path,
                method: parts.method,
                uri: parts.uri,
                query_string,
                headers: parts.headers,
                body: body_bytes,
                remote_addr,
                document_root: route_config.document_root_arc(),
            };

            let response_rx = executor.execute(script_request);

            match response_rx.await {
                Ok(script_response) => {
                    let mut builder = Response::builder().status(script_response.status);

                    for (name, value) in &script_response.headers {
                        builder = builder.header(name, value);
                    }

                    builder.body(full_body(script_response.body))?
                }
                Err(_) => Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .body(full_body(Bytes::from_static(b"500 PHP Worker Error")))?,
            }
        }
        RouteResult::NotFound => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(full_body(Bytes::from_static(b"404 Not Found")))?,
    };

    Ok(response)
}

fn rand_id() -> u64 {
    use std::time::SystemTime;
    let seed = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    // Simple hash for request ID — not cryptographic, just unique enough for tracing
    seed.wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407)
}
