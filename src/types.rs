use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Uri};
use http_body_util::{combinators::BoxBody, BodyExt, Full};

/// Convenience alias for a boxed error that is Send + Sync.
/// Used throughout the codebase for fallible operations.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Unified response body type supporting both buffered and streaming responses.
/// Uses `std::io::Error` as the error type to be compatible with `ReaderStream`.
pub type ResponseBody = BoxBody<Bytes, std::io::Error>;

/// Create a `ResponseBody` from a `Bytes` value (buffered, non-streaming).
pub fn full_body(bytes: Bytes) -> ResponseBody {
    Full::new(bytes).map_err(|never| match never {}).boxed()
}

/// Request sent from Tokio task to PHP worker thread.
#[derive(Debug)]
pub struct ScriptRequest {
    pub request_id: String,
    pub script_path: PathBuf,
    pub method: Method,
    pub uri: Uri,
    pub query_string: String,
    pub headers: HeaderMap,
    pub body: Bytes,
    pub remote_addr: SocketAddr,
    pub document_root: Arc<PathBuf>,
}

/// Response sent from PHP worker thread back to Tokio task.
#[derive(Debug)]
pub struct ScriptResponse {
    pub status: u16,
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub body: Bytes,
    pub execution_time_us: u64,
}

impl Default for ScriptResponse {
    fn default() -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: Bytes::new(),
            execution_time_us: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_script_response_default() {
        let resp = ScriptResponse::default();
        assert_eq!(resp.status, 200);
        assert!(resp.headers.is_empty());
        assert!(resp.body.is_empty());
        assert_eq!(resp.execution_time_us, 0);
    }
}
