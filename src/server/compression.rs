use bytes::Bytes;
use http::{header, HeaderValue, Response};
use http_body_util::BodyExt;

use crate::types::{full_body, ResponseBody};

/// Minimum body size worth compressing (bytes).
const MIN_COMPRESS_SIZE: usize = 256;

/// Maximum body size to attempt compression (3 MB).
/// Larger responses should be streamed from disk without compression.
const MAX_COMPRESS_SIZE: usize = 3 * 1024 * 1024;

/// Brotli window size. 20 = 1 MB window — good for typical web responses.
const BROTLI_WINDOW: i32 = 20;

/// Bodies smaller than this are compressed inline on the async thread.
/// Above this threshold, compression is offloaded to a blocking thread
/// to avoid stalling the Tokio runtime.
const BLOCKING_THRESHOLD_LOW: usize = 65_536;

/// Lower threshold for high quality levels (>4) where brotli is 10-100x slower.
/// At q11, even 8 KB can take milliseconds — keep async thread responsive.
const BLOCKING_THRESHOLD_HIGH: usize = 4_096;

/// Try to compress a response with brotli. Called only when the client accepts brotli.
/// The caller must verify `accepts_brotli()` before calling.
/// `quality` is the brotli compression level (1-11).
pub async fn maybe_compress(
    response: Response<ResponseBody>,
    quality: i32,
) -> Response<ResponseBody> {
    // 206 bodies must not be re-encoded: Content-Range offsets refer to the
    // unencoded representation (RFC 9110 §14.4), so compressing a partial
    // response would corrupt client-side range reassembly.
    if response.status() == http::StatusCode::PARTIAL_CONTENT {
        return response;
    }

    // Check Content-Type
    let content_type = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");

    if !is_compressible(content_type) {
        return response;
    }

    // Don't double-compress
    if response.headers().contains_key(header::CONTENT_ENCODING) {
        return response;
    }

    // Pre-check Content-Length to avoid collecting oversized bodies into memory
    if let Some(cl) = response.headers().get(header::CONTENT_LENGTH) {
        if let Ok(len) = cl.to_str().unwrap_or("0").parse::<usize>() {
            if !(MIN_COMPRESS_SIZE..=MAX_COMPRESS_SIZE).contains(&len) {
                return response;
            }
        }
    }

    // Split response and check body size hint before collecting
    let (parts, body) = response.into_parts();

    // Streaming bodies have no upper size bound: compressing one would mean
    // buffering the entire stream in memory until it ends, destroying
    // time-to-first-byte for flush()-style PHP responses. Pass them through.
    // Buffered bodies (`Full`) always report an exact upper bound.
    let Some(upper) = hyper::body::Body::size_hint(&body).upper() else {
        return Response::from_parts(parts, body);
    };
    if !(MIN_COMPRESS_SIZE as u64..=MAX_COMPRESS_SIZE as u64).contains(&upper) {
        return Response::from_parts(parts, body);
    }

    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return Response::from_parts(parts, full_body(Bytes::new())),
    };

    // Runtime check for bodies without exact size hint
    if body_bytes.len() < MIN_COMPRESS_SIZE || body_bytes.len() > MAX_COMPRESS_SIZE {
        return Response::from_parts(parts, full_body(body_bytes));
    }

    // Small bodies: compress inline (spawn_blocking overhead > compression time).
    // Large bodies: offload to blocking thread to avoid stalling the runtime.
    // High quality levels (>4) use a lower threshold since brotli gets much slower.
    let blocking_threshold = if quality <= 4 {
        BLOCKING_THRESHOLD_LOW
    } else {
        BLOCKING_THRESHOLD_HIGH
    };
    let compressed = if body_bytes.len() <= blocking_threshold {
        match compress_brotli(&body_bytes, quality) {
            Some(c) => c,
            None => return Response::from_parts(parts, full_body(body_bytes)),
        }
    } else {
        let body_bytes_ref = body_bytes.clone();
        match tokio::task::spawn_blocking(move || compress_brotli(&body_bytes_ref, quality)).await {
            Ok(Some(c)) => c,
            Ok(None) => return Response::from_parts(parts, full_body(body_bytes)),
            Err(e) => {
                tracing::warn!(error = %e, "Compression spawn_blocking task failed");
                return Response::from_parts(parts, full_body(body_bytes));
            }
        }
    };

    let compressed_len = compressed.len();
    let mut response = Response::from_parts(parts, full_body(Bytes::from(compressed)));
    response
        .headers_mut()
        .insert(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
    // The compressed bytes are a different representation than the identity
    // bytes the ETag was computed from; a strong tag shared by both would let
    // a client resume a brotli download with identity 206 fragments
    // (If-Range requires strong comparison, RFC 9110 §13.1.5). Downgrade to
    // weak — nginx's gzip filter does the same. Weak tags still revalidate
    // via If-None-Match, which uses weak comparison.
    let weakened = response
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .filter(|tag| !tag.starts_with("W/"))
        .and_then(|tag| HeaderValue::from_str(&format!("W/{tag}")).ok());
    if let Some(weak_etag) = weakened {
        response.headers_mut().insert(header::ETAG, weak_etag);
    }
    // Drop Accept-Ranges: byte offsets are meaningless against the encoded
    // body, and a date-form If-Range (which the weak ETag cannot guard)
    // would otherwise let a client resume this brotli download with
    // identity 206 fragments. nginx's gzip filter clears the header the
    // same way (ngx_http_clear_accept_ranges).
    response.headers_mut().remove(header::ACCEPT_RANGES);
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from(compressed_len));
    // Append Vary: Accept-Encoding so caches store both compressed and uncompressed
    response
        .headers_mut()
        .append(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    response
}

pub(crate) fn accepts_brotli(accept_encoding: &str) -> bool {
    accept_encoding.split(',').any(|enc| {
        let name = enc.trim().split(';').next().unwrap_or("").trim();
        name == "br"
    })
}

/// Check if the MIME type should be compressed.
fn is_compressible(content_type: &str) -> bool {
    let ct = content_type.split(';').next().unwrap_or("").trim();
    matches!(
        ct,
        "text/html"
            | "text/css"
            | "text/plain"
            | "text/xml"
            | "text/javascript"
            | "application/javascript"
            | "application/json"
            | "application/xml"
            | "application/xhtml+xml"
            | "application/rss+xml"
            | "application/atom+xml"
            | "application/manifest+json"
            | "application/ld+json"
            | "application/wasm"
            | "image/svg+xml"
            | "font/ttf"
            | "font/otf"
            | "application/x-font-ttf"
            | "application/x-font-opentype"
            | "application/vnd.ms-fontobject"
    )
}

/// Compress data using Brotli. Returns None if compression would not reduce size.
fn compress_brotli(data: &[u8], quality: i32) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(data.len() / 2);
    let params = brotli::enc::BrotliEncoderParams {
        quality,
        lgwin: BROTLI_WINDOW,
        ..Default::default()
    };
    match brotli::BrotliCompress(&mut &data[..], &mut output, &params) {
        Ok(_) if output.len() < data.len() => Some(output),
        Ok(_) => None, // compressed is not smaller
        Err(e) => {
            tracing::debug!(error = %e, "Brotli compression failed");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::StatusCode;

    fn build_response(content_type: &str, body: &[u8]) -> Response<ResponseBody> {
        Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, content_type)
            .body(full_body(Bytes::from(body.to_vec())))
            .unwrap()
    }

    #[test]
    fn test_accepts_brotli() {
        assert!(accepts_brotli("br"));
        assert!(accepts_brotli("gzip, br"));
        assert!(accepts_brotli("gzip, deflate, br;q=1.0"));
        assert!(accepts_brotli("br;q=0.5, gzip"));
        assert!(!accepts_brotli("gzip"));
        assert!(!accepts_brotli("deflate"));
        assert!(!accepts_brotli(""));
        assert!(!accepts_brotli("brand")); // must not match prefix
    }

    #[test]
    fn test_is_compressible() {
        // Text types
        assert!(is_compressible("text/html"));
        assert!(is_compressible("text/css"));
        assert!(is_compressible("text/plain"));
        assert!(is_compressible("text/xml"));
        assert!(is_compressible("text/javascript"));
        assert!(is_compressible("text/html; charset=utf-8"));

        // Application types
        assert!(is_compressible("application/json"));
        assert!(is_compressible("application/javascript"));
        assert!(is_compressible("application/xml"));
        assert!(is_compressible("application/xhtml+xml"));
        assert!(is_compressible("application/rss+xml"));
        assert!(is_compressible("application/atom+xml"));
        assert!(is_compressible("application/manifest+json"));
        assert!(is_compressible("application/ld+json"));
        assert!(is_compressible("application/wasm"));

        // Other compressible
        assert!(is_compressible("image/svg+xml"));
        assert!(is_compressible("font/ttf"));
        assert!(is_compressible("font/otf"));

        // Not compressible
        assert!(!is_compressible("image/png"));
        assert!(!is_compressible("image/jpeg"));
        assert!(!is_compressible("application/octet-stream"));
        assert!(!is_compressible("video/mp4"));
        assert!(!is_compressible("font/woff"));
        assert!(!is_compressible("font/woff2"));
        assert!(!is_compressible("application/zip"));
    }

    #[test]
    fn test_compress_brotli_roundtrip() {
        let data = "Hello, World! ".repeat(50);
        let compressed = compress_brotli(data.as_bytes(), 4);
        assert!(compressed.is_some());

        let compressed = compressed.unwrap();
        let mut decompressed = Vec::new();
        brotli::BrotliDecompress(&mut &compressed[..], &mut decompressed).unwrap();
        assert_eq!(decompressed, data.as_bytes());
    }

    #[test]
    fn test_compress_brotli_returns_none_when_not_smaller() {
        // Random-ish data that doesn't compress well
        let data: Vec<u8> = (0..300).map(|i| (i * 7 + 13) as u8).collect();
        let result = compress_brotli(&data, 4);
        if let Some(compressed) = result {
            assert!(compressed.len() < data.len());
        }
    }

    #[tokio::test]
    async fn test_compress_html_response() {
        let body = "a".repeat(500); // >256 bytes, compressible
        let response = build_response("text/html", body.as_bytes());

        let result = maybe_compress(response, 4).await;

        assert_eq!(
            result.headers().get(header::CONTENT_ENCODING).unwrap(),
            "br"
        );
        assert!(result.headers().get(header::VARY).is_some());

        // Verify Content-Length matches compressed body
        let cl: usize = result
            .headers()
            .get(header::CONTENT_LENGTH)
            .unwrap()
            .to_str()
            .unwrap()
            .parse()
            .unwrap();
        assert!(cl < body.len());

        let collected = result.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(collected.len(), cl);
    }

    #[tokio::test]
    async fn test_skip_non_compressible_type() {
        let body = vec![0u8; 500];
        let response = build_response("image/png", &body);

        let result = maybe_compress(response, 4).await;

        assert!(result.headers().get(header::CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn test_skip_small_body() {
        let response = build_response("text/html", b"<h1>Hi</h1>");

        let result = maybe_compress(response, 4).await;

        assert!(result.headers().get(header::CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn test_skip_already_encoded() {
        let body = "x".repeat(500);
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html")
            .header(header::CONTENT_ENCODING, "gzip")
            .body(full_body(Bytes::from(body)))
            .unwrap();

        let result = maybe_compress(response, 4).await;

        assert_eq!(
            result.headers().get(header::CONTENT_ENCODING).unwrap(),
            "gzip"
        );
    }

    #[tokio::test]
    async fn test_skip_streaming_body() {
        // Streaming bodies (unknown upper size hint) must never be compressed —
        // compressing would require buffering the whole stream.
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        tx.send(Bytes::from("x".repeat(500))).await.unwrap();
        drop(tx);

        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html")
            .body(crate::types::stream_body(Bytes::new(), rx))
            .unwrap();

        let result = maybe_compress(response, 4).await;

        assert!(result.headers().get(header::CONTENT_ENCODING).is_none());
        let collected = result.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(collected.len(), 500);
    }

    #[tokio::test]
    async fn test_streaming_body_not_buffered() {
        // The sender stays open: if maybe_compress buffered the stream it
        // would await channel close and never return within the timeout.
        let (tx, rx) = tokio::sync::mpsc::channel::<Bytes>(4);

        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html")
            .body(crate::types::stream_body(Bytes::from_static(b"first"), rx))
            .unwrap();

        let result = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            maybe_compress(response, 4),
        )
        .await;

        assert!(
            result.is_ok(),
            "maybe_compress buffered a live stream instead of passing it through"
        );
        drop(tx);
    }

    #[tokio::test]
    async fn test_compress_weakens_strong_etag() {
        // The br body is a different representation than the identity bytes
        // the strong ETag described — the tag must be downgraded to weak so
        // If-Range can never strong-match across representations.
        let body = "a".repeat(500);
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html")
            .header(header::ETAG, "\"500-abc\"")
            .header(header::ACCEPT_RANGES, "bytes")
            .body(full_body(Bytes::from(body)))
            .unwrap();

        let result = maybe_compress(response, 4).await;

        assert_eq!(
            result.headers().get(header::CONTENT_ENCODING).unwrap(),
            "br"
        );
        assert_eq!(result.headers().get(header::ETAG).unwrap(), "W/\"500-abc\"");
        // Byte offsets don't apply to the encoded body, and a date-form
        // If-Range cannot be guarded by the weakened ETag — advertise no
        // range support for this representation (nginx gzip-filter behavior).
        assert!(result.headers().get(header::ACCEPT_RANGES).is_none());
    }

    #[tokio::test]
    async fn test_uncompressed_keeps_strong_etag() {
        // When compression is skipped the representation is unchanged —
        // the strong ETag must survive so If-Range resume keeps working.
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "image/png") // not compressible
            .header(header::ETAG, "\"500-abc\"")
            .header(header::ACCEPT_RANGES, "bytes")
            .body(full_body(Bytes::from(vec![0u8; 500])))
            .unwrap();

        let result = maybe_compress(response, 4).await;

        assert!(result.headers().get(header::CONTENT_ENCODING).is_none());
        assert_eq!(result.headers().get(header::ETAG).unwrap(), "\"500-abc\"");
        assert_eq!(
            result.headers().get(header::ACCEPT_RANGES).unwrap(),
            "bytes"
        );
    }

    #[tokio::test]
    async fn test_skip_partial_content() {
        // 206 must never be compressed: Content-Range refers to unencoded bytes.
        let body = "a".repeat(500); // would otherwise qualify for compression
        let response = Response::builder()
            .status(StatusCode::PARTIAL_CONTENT)
            .header(header::CONTENT_TYPE, "text/html")
            .header(header::CONTENT_RANGE, "bytes 0-499/1000")
            .body(full_body(Bytes::from(body)))
            .unwrap();

        let result = maybe_compress(response, 4).await;

        assert!(result.headers().get(header::CONTENT_ENCODING).is_none());
        let collected = result.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(collected.len(), 500);
    }

    #[tokio::test]
    async fn test_skip_large_content_length() {
        let body = "x".repeat(500);
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html")
            .header(header::CONTENT_LENGTH, "10000000") // 10 MB — exceeds MAX_COMPRESS_SIZE
            .body(full_body(Bytes::from(body)))
            .unwrap();

        let result = maybe_compress(response, 4).await;

        assert!(result.headers().get(header::CONTENT_ENCODING).is_none());
    }
}
