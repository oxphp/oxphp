use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use bytes::Bytes;
use futures_util::StreamExt;
use http::{HeaderMap, HeaderName, HeaderValue, Method, Uri, Version};
use http_body_util::{combinators::BoxBody, BodyExt, Full, StreamBody};
use hyper::body::Frame;

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

/// Create a streaming `ResponseBody` from an optional first chunk and an mpsc receiver.
/// The first chunk (if non-empty) is sent before draining the channel.
/// The stream ends when the channel is closed (sender dropped).
pub fn stream_body(first_chunk: Bytes, rx: tokio::sync::mpsc::Receiver<Bytes>) -> ResponseBody {
    let rx_stream =
        tokio_stream::wrappers::ReceiverStream::new(rx).map(Ok::<Bytes, std::io::Error>);

    if first_chunk.is_empty() {
        BodyExt::boxed(StreamBody::new(
            rx_stream.map(|r: Result<Bytes, std::io::Error>| r.map(Frame::data)),
        ))
    } else {
        let first =
            futures_util::stream::once(async move { Ok::<Bytes, std::io::Error>(first_chunk) });
        let combined = first.chain(rx_stream);
        BodyExt::boxed(StreamBody::new(
            combined.map(|r: Result<Bytes, std::io::Error>| r.map(Frame::data)),
        ))
    }
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
    /// Execution deadline in microseconds (0 = no deadline).
    pub timeout_us: u64,
    /// W3C Trace Context: trace ID (32 hex chars, empty if tracing disabled).
    pub trace_id: String,
    /// W3C Trace Context: span ID (16 hex chars, empty if tracing disabled).
    pub span_id: String,
    /// W3C Trace Context: parent span ID (16 hex chars or empty).
    pub parent_span_id: String,
    /// Whether this request arrived over TLS.
    pub is_tls: bool,
    /// HTTP version of the request (e.g., HTTP/1.0, HTTP/1.1, HTTP/2).
    pub version: Version,
    /// Extra path after the `.php` script (e.g. `/user/42` for `/app.php/user/42`).
    /// Set when `SPLIT_PATH_INFO_ENABLED=true` and the URI contains a `.php` prefix.
    pub path_info: Option<String>,
}

/// A PHP error captured during script execution.
#[derive(Debug, Clone)]
pub struct PhpError {
    /// Severity: "error", "warn", "info" (matches tracing level from error_type_str).
    pub level: &'static str,
    /// PHP error constant name: "E_ERROR", "E_WARNING", etc.
    pub error_type: &'static str,
    pub message: String,
    pub file: String,
    pub line: u32,
    /// Stack trace for exceptions and fatal errors.
    pub stacktrace: Option<String>,
}

/// Response sent from PHP worker thread back to Tokio task.
pub struct ScriptResponse {
    pub status: u16,
    pub headers: Vec<(HeaderName, HeaderValue)>,
    pub body: Bytes,
    pub execution_time_us: u64,
    /// If Some, response is streaming: `body` is the first chunk (may be empty),
    /// subsequent chunks arrive via this channel. Channel close = stream end.
    pub stream_rx: Option<tokio::sync::mpsc::Receiver<Bytes>>,
    /// PHP errors captured during script execution.
    pub errors: Vec<PhpError>,
    /// Serialized APM child spans (JSON). Populated by SpanStack drain on PHP worker thread.
    /// None when APM is disabled or no spans were created.
    pub apm_spans_json: Option<String>,
}

impl std::fmt::Debug for ScriptResponse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScriptResponse")
            .field("status", &self.status)
            .field("headers", &self.headers)
            .field("body_len", &self.body.len())
            .field("execution_time_us", &self.execution_time_us)
            .field("streaming", &self.stream_rx.is_some())
            .field("errors_count", &self.errors.len())
            .field("has_apm_spans", &self.apm_spans_json.is_some())
            .finish()
    }
}

impl Default for ScriptResponse {
    fn default() -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: Bytes::new(),
            execution_time_us: 0,
            stream_rx: None,
            errors: Vec::new(),
            apm_spans_json: None,
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
        assert!(resp.stream_rx.is_none());
        assert!(resp.errors.is_empty());
    }

    #[tokio::test]
    async fn test_stream_body_with_first_chunk() {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.send(Bytes::from_static(b"chunk2")).await.unwrap();
        drop(tx);

        let body = stream_body(Bytes::from_static(b"chunk1"), rx);
        let collected = body.collect().await.unwrap().to_bytes();
        assert_eq!(&collected[..], b"chunk1chunk2");
    }

    #[tokio::test]
    async fn test_stream_body_empty_first_chunk() {
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.send(Bytes::from_static(b"only")).await.unwrap();
        drop(tx);

        let body = stream_body(Bytes::new(), rx);
        let collected = body.collect().await.unwrap().to_bytes();
        assert_eq!(&collected[..], b"only");
    }

    #[tokio::test]
    async fn test_stream_body_empty() {
        let (_tx, rx) = tokio::sync::mpsc::channel::<Bytes>(4);
        drop(_tx);

        let body = stream_body(Bytes::new(), rx);
        let collected = body.collect().await.unwrap().to_bytes();
        assert!(collected.is_empty());
    }
}
