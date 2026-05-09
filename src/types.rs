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
#[inline]
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
    /// Per-request cancellation state shared with the dispatch task.
    /// Holding an Arc on this side keeps the AtomicU8 alive even if
    /// the tokio future is dropped before the worker finishes.
    pub cancel_state: std::sync::Arc<crate::bridge::cancel::CancellationState>,
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
    /// Extra path after the script component. In Traditional mode this is the
    /// segment after a `.php` prefix (e.g. `/user/42` for `/app.php/user/42`).
    /// In Framework mode this is the full original URI rewritten onto the
    /// front controller.
    pub path_info: Option<String>,
    /// Original protocol from trusted proxy (e.g. "https").
    /// Set from `Forwarded: proto=` or `X-Forwarded-Proto` header.
    pub forwarded_proto: Option<String>,
    /// Original host from trusted proxy (e.g. "example.com:8443").
    /// Set from `Forwarded: host=` or `X-Forwarded-Host` header.
    pub forwarded_host: Option<String>,
    /// Metadata for `$_SERVER['OXPHP_DENIED_*']` population — set only when
    /// this request was routed here by the `PHP_DENY_DIRS` fallback.
    /// Boxed behind `Arc` so the field stays 8 bytes when `None` (the
    /// dominant case) and the rare-path clone is one atomic increment
    /// instead of three String clones.
    pub denied_meta: Option<Arc<crate::config::DeniedMeta>>,
    /// Profiling mode selected for this request. The profiler plugin (or any
    /// future mode-aware plugin) writes this on the Tokio thread via
    /// `PluginRequestActions::set_profiling_decision`; the worker thread reads
    /// it and passes it into `ProfilingContext::reset` at RINIT.
    pub profiling_mode: crate::profiling::ProfilingMode,
    /// Run identifier minted by the profiler when `ProfileAll` is selected.
    /// Used by future PRs for storage / export correlation.
    pub profiling_run_id: Option<String>,
}

/// A PHP error captured during script execution (E_ERROR/E_WARNING/E_NOTICE, exceptions).
///
/// Distinct from `crate::plugin::PhpError` (enum for plugin-function call errors).
#[derive(Debug, Clone)]
pub struct PhpScriptError {
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
    pub errors: Vec<PhpScriptError>,
    /// Finalized span tree for the request. Produced by `ProfilingContext::finalize()` on the
    /// PHP worker thread. `None` when APM is disabled or no spans were created.
    pub profile_tree: Option<std::sync::Arc<crate::profiling::SpanTree>>,
    /// Cancellation reason observed at response-send time, mirrored from
    /// the per-request `CancellationState`. 0 = no cancellation.
    /// Used by the dispatch side to bump
    /// `oxphp_request_cancelled_total{reason}`.
    pub cancel_reason: u8,
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
            .field("has_profile_tree", &self.profile_tree.is_some())
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
            profile_tree: None,
            cancel_reason: 0,
        }
    }
}

impl ScriptResponse {
    /// 499 Client Closed Request — nginx convention for "client gave up".
    /// Used by the worker fast-path when a queued request's client has
    /// disconnected before any PHP runs.
    pub fn client_closed() -> Self {
        Self {
            status: 499,
            cancel_reason: crate::bridge::cancel::CancelReason::ClientAbort as u8,
            ..Default::default()
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
