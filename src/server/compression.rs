use bytes::Bytes;
use http::{header, Response};
use http_body_util::BodyExt;

use crate::types::{full_body, ResponseBody};

/// Minimum body size worth compressing (bytes).
const MIN_COMPRESS_SIZE: usize = 256;

/// Maximum body size to attempt compression (3 MB).
/// Larger responses should be streamed from disk without compression.
const MAX_COMPRESS_SIZE: usize = 3 * 1024 * 1024;

/// Brotli quality level (1-11). 4 is a good balance for web serving.
const BROTLI_QUALITY: i32 = 4;

/// Brotli window size. 20 = 1 MB window — good for typical web responses.
const BROTLI_WINDOW: i32 = 20;

/// Try to compress a response with brotli. Called only when Accept-Encoding is present.
/// The caller must check for None accept_encoding before calling to avoid async overhead.
pub async fn maybe_compress(
    response: Response<ResponseBody>,
    accept_encoding: &str,
) -> Response<ResponseBody> {
    if !accepts_brotli(accept_encoding) {
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

    // Use body size hint to skip collect when size is known upfront
    if let Some(upper) = hyper::body::Body::size_hint(&body).upper() {
        if !(MIN_COMPRESS_SIZE as u64..=MAX_COMPRESS_SIZE as u64).contains(&upper) {
            return Response::from_parts(parts, body);
        }
    }

    let body_bytes = match body.collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(_) => return Response::from_parts(parts, full_body(Bytes::new())),
    };

    // Runtime check for bodies without exact size hint
    if body_bytes.len() < MIN_COMPRESS_SIZE || body_bytes.len() > MAX_COMPRESS_SIZE {
        return Response::from_parts(parts, full_body(body_bytes));
    }

    // Compress with brotli — returns None if compressed is not smaller
    let compressed = match compress_brotli(&body_bytes) {
        Some(c) => c,
        None => return Response::from_parts(parts, full_body(body_bytes)),
    };

    let compressed_len = compressed.len();
    let mut response = Response::from_parts(parts, full_body(Bytes::from(compressed)));
    response
        .headers_mut()
        .insert(header::CONTENT_ENCODING, "br".parse().unwrap());
    response.headers_mut().insert(
        header::CONTENT_LENGTH,
        compressed_len.to_string().parse().unwrap(),
    );
    // Append Vary: Accept-Encoding so caches store both compressed and uncompressed
    response
        .headers_mut()
        .append(header::VARY, "Accept-Encoding".parse().unwrap());
    response
}

fn accepts_brotli(accept_encoding: &str) -> bool {
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
fn compress_brotli(data: &[u8]) -> Option<Vec<u8>> {
    let mut output = Vec::with_capacity(data.len() / 2);
    let params = brotli::enc::BrotliEncoderParams {
        quality: BROTLI_QUALITY,
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
        let compressed = compress_brotli(data.as_bytes());
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
        let result = compress_brotli(&data);
        if let Some(compressed) = result {
            assert!(compressed.len() < data.len());
        }
    }

    #[tokio::test]
    async fn test_compress_html_response() {
        let body = "a".repeat(500); // >256 bytes, compressible
        let response = build_response("text/html", body.as_bytes());

        let result = maybe_compress(response, "gzip, br").await;

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
    async fn test_skip_when_no_brotli_support() {
        let body = "x".repeat(500);
        let response = build_response("text/html", body.as_bytes());

        let result = maybe_compress(response, "gzip, deflate").await;

        assert!(result.headers().get(header::CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn test_skip_non_compressible_type() {
        let body = vec![0u8; 500];
        let response = build_response("image/png", &body);

        let result = maybe_compress(response, "br").await;

        assert!(result.headers().get(header::CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn test_skip_small_body() {
        let response = build_response("text/html", b"<h1>Hi</h1>");

        let result = maybe_compress(response, "br").await;

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

        let result = maybe_compress(response, "br").await;

        assert_eq!(
            result.headers().get(header::CONTENT_ENCODING).unwrap(),
            "gzip"
        );
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

        let result = maybe_compress(response, "br").await;

        assert!(result.headers().get(header::CONTENT_ENCODING).is_none());
    }
}
