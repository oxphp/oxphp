use bytes::Bytes;
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
