use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use hyper::body::Incoming;

use crate::server::response::static_file::{self, FileCache};
use crate::server::routing::{RouteConfig, RouteResult};
use crate::types::{full_body, ResponseBody};

/// Handle a single HTTP request: resolve route, serve static file or return 404.
/// Returns `Infallible` error type — errors are converted to HTTP 500 responses.
pub async fn handle_request(
    req: Request<Incoming>,
    route_config: &RouteConfig,
    file_cache: &Arc<FileCache>,
    remote_addr: SocketAddr,
) -> Result<Response<ResponseBody>, Infallible> {
    let uri = req.uri().clone();

    tracing::debug!(
        method = %req.method(),
        uri = %uri,
        remote_addr = %remote_addr,
        "Handling request"
    );

    let response = match handle_request_inner(uri.path(), route_config, file_cache).await {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!(error = %e, uri = %uri, "Internal server error");
            Response::builder()
                .status(StatusCode::INTERNAL_SERVER_ERROR)
                .body(full_body(Bytes::from_static(b"500 Internal Server Error")))
                .unwrap()
        }
    };

    Ok(response)
}

async fn handle_request_inner(
    uri_path: &str,
    route_config: &RouteConfig,
    file_cache: &Arc<FileCache>,
) -> Result<Response<ResponseBody>, crate::types::BoxError> {
    let route_result = route_config.resolve_request(uri_path, file_cache).await;

    let response = match route_result {
        RouteResult::Serve(file_path) => static_file::serve(&file_path, file_cache).await?,
        RouteResult::Execute(_script_path) => {
            // Phase 2: PHP execution — return 404 for now
            Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(full_body(Bytes::from_static(b"404 Not Found")))?
        }
        RouteResult::NotFound => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(full_body(Bytes::from_static(b"404 Not Found")))?,
    };

    Ok(response)
}
