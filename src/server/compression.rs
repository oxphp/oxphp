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

/// A content coding this server can produce.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Coding {
    Br,
    Gzip,
    Zstd,
}

impl Coding {
    /// Every coding, in the order their per-coding arrays are indexed.
    pub(crate) const ALL: [Coding; 3] = [Coding::Br, Coding::Gzip, Coding::Zstd];

    /// How many codings exist. The width of a per-coding array.
    pub(crate) const COUNT: usize = Self::ALL.len();

    /// Server preference for a body compressed while the request waits, best
    /// first. Zstandard leads because it buys nearly all of brotli's
    /// compression for a fraction of its cost: over real bodies above 4 KB it
    /// came within a few percent of brotli's per-request quality on size while
    /// spending well under half the CPU, and that ratio is better on x86-64
    /// than on arm64. Brotli sits ahead of gzip because at the quality it runs
    /// at here it is smaller than gzip on every body measured.
    const PER_REQUEST: [Coding; Self::COUNT] = [Coding::Zstd, Coding::Br, Coding::Gzip];

    /// Server preference for a cached static file, best first. Brotli leads
    /// here for the opposite reason: its cost is paid once and never again, and
    /// at the top of its range — with the static dictionary that makes it slow —
    /// it produces the smallest bytes of the three.
    const ARTIFACT: [Coding; Self::COUNT] = [Coding::Br, Coding::Zstd, Coding::Gzip];

    /// The token this coding is named by in `Accept-Encoding` and
    /// `Content-Encoding`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Coding::Br => "br",
            Coding::Gzip => "gzip",
            Coding::Zstd => "zstd",
        }
    }

    /// Read a coding out of configuration. `brotli` is accepted alongside the
    /// header token `br`, which reads as a typo in a settings file.
    pub fn from_name(name: &str) -> Option<Coding> {
        Coding::ALL
            .into_iter()
            .find(|coding| name.eq_ignore_ascii_case(coding.name()))
            .or_else(|| name.eq_ignore_ascii_case("brotli").then_some(Coding::Br))
    }

    /// This coding's index in a per-coding array.
    pub(crate) fn slot(self) -> usize {
        self as usize
    }
}

/// The level each coding is configured to run at. Zero means the server does
/// not offer that coding at all — either its level was set to zero or it was
/// left out of the offered set.
#[derive(Clone, Copy, Debug, Default)]
pub struct Levels {
    /// Brotli quality, 0-11.
    pub brotli: i32,
    /// Gzip level, 0-9.
    pub gzip: i32,
    /// Zstandard level, 0-19.
    pub zstd: i32,
}

impl Levels {
    /// Set one coding's level. Zero withdraws the coding.
    pub(crate) fn set(&mut self, coding: Coding, level: i32) {
        match coding {
            Coding::Br => self.brotli = level,
            Coding::Gzip => self.gzip = level,
            Coding::Zstd => self.zstd = level,
        }
    }

    /// The configured level for one coding.
    pub(crate) fn level(&self, coding: Coding) -> i32 {
        match coding {
            Coding::Br => self.brotli,
            Coding::Gzip => self.gzip,
            Coding::Zstd => self.zstd,
        }
    }
}

/// What one client will accept, weighed against what this server offers.
///
/// Read from `Accept-Encoding` once per request and then asked twice, because
/// the answer depends on what happens to the compressed bytes afterwards: a
/// response encoded for this request alone wants the cheapest coding that is
/// small enough, while a static file whose compressed copy will be served
/// thousands of times wants the smallest coding whatever it costs. The client's
/// weights outrank both — RFC 9110 §12.5.3 makes them a ranking, not a list —
/// and server preference only breaks a tie, which is the usual case since
/// browsers send every coding they support at the default weight.
#[derive(Clone, Copy, Debug, Default)]
pub struct Acceptable([u16; Coding::COUNT]);

impl Acceptable {
    /// Weigh one `Accept-Encoding` field against the server's configuration.
    pub(crate) fn parse(accept_encoding: &str, levels: Levels) -> Self {
        let mut weights = [0u16; Coding::COUNT];
        for coding in Coding::ALL {
            if levels.level(coding) > 0 {
                weights[coding.slot()] = qvalue(accept_encoding, coding.name());
            }
        }
        Self(weights)
    }

    /// The coding to encode a response with on the request path, or `None` for
    /// identity.
    pub(crate) fn per_request(self) -> Option<Coding> {
        self.best(Coding::PER_REQUEST)
    }

    /// The coding whose stored copy of a cached static file this client should
    /// be served, or `None` for identity.
    pub(crate) fn artifact(self) -> Option<Coding> {
        self.best(Coding::ARTIFACT)
    }

    fn best(self, preference: [Coding; Coding::COUNT]) -> Option<Coding> {
        let mut best: Option<(u16, Coding)> = None;
        for coding in preference {
            // Zero is a refusal, and a coding the header never named scores
            // zero unless a `*` covers it.
            let weight = self.0[coding.slot()];
            if weight > 0 && best.is_none_or(|(best_weight, _)| weight > best_weight) {
                best = Some((weight, coding));
            }
        }
        best.map(|(_, coding)| coding)
    }
}

/// Try to compress a response with `coding`, which the caller must have
/// obtained from [`negotiate`] for this client. `level` is that coding's
/// configured level.
pub async fn maybe_compress(
    response: Response<ResponseBody>,
    coding: Coding,
    level: i32,
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

    // A body with no upper size bound is passed through. Not because the
    // codings cannot encode a stream — all three have incremental encoders,
    // and a flush per chunk is what nginx's gzip filter has always done — but
    // because this function is one-shot by construction: it takes a whole body
    // and returns a whole body. Encoding incrementally would mean an encoder
    // held open per connection for as long as the stream lasts.
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
    let compressed = if body_bytes.len() <= blocking_threshold(coding, level) {
        match compress(&body_bytes, coding, level) {
            Some(c) => c,
            None => return Response::from_parts(parts, full_body(body_bytes)),
        }
    } else {
        let body_bytes_ref = body_bytes.clone();
        match tokio::task::spawn_blocking(move || compress(&body_bytes_ref, coding, level)).await {
            Ok(Some(c)) => c,
            Ok(None) => return Response::from_parts(parts, full_body(body_bytes)),
            Err(e) => {
                tracing::warn!(error = %e, "Compression spawn_blocking task failed");
                return Response::from_parts(parts, full_body(body_bytes));
            }
        }
    };

    let mut response = Response::from_parts(parts, full_body(Bytes::from(compressed)));
    mark_encoded(&mut response, coding);
    response
}

/// The body size above which compressing at this level goes to the blocking
/// pool. Every coding is cheap enough at its lower levels that a 64 KB body
/// costs well under a millisecond inline; all of them climb steeply higher up
/// their range, where even a few kilobytes is worth handing off. Brotli's knee
/// is the hasher change at 5, which is also the level it ships at, so brotli
/// hands off from 4 KB by default — the CPU that buys its ratio is exactly the
/// CPU an async thread must not spend.
fn blocking_threshold(coding: Coding, level: i32) -> usize {
    let steep = match coding {
        Coding::Br => level > 4,
        Coding::Gzip => level > 6,
        Coding::Zstd => level > 9,
    };
    if steep {
        BLOCKING_THRESHOLD_HIGH
    } else {
        BLOCKING_THRESHOLD_LOW
    }
}

/// Rewrite the headers of a response whose body has just been replaced with an
/// encoded representation. Both compression paths end here — the one that
/// encodes on the request, and static serving handing over a cached artifact —
/// so an encoded response looks the same however it was produced.
pub(crate) fn mark_encoded(response: &mut Response<ResponseBody>, coding: Coding) {
    let encoded_len = hyper::body::Body::size_hint(response.body())
        .exact()
        .unwrap_or(0);
    response.headers_mut().insert(
        header::CONTENT_ENCODING,
        HeaderValue::from_static(coding.name()),
    );
    // The encoded bytes are a different representation than the identity
    // bytes the ETag was computed from; a strong tag shared by both would let
    // a client resume an encoded download with identity 206 fragments
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
    // would otherwise let a client resume this encoded download with
    // identity 206 fragments. nginx's gzip filter clears the header the
    // same way (ngx_http_clear_accept_ranges).
    response.headers_mut().remove(header::ACCEPT_RANGES);
    response
        .headers_mut()
        .insert(header::CONTENT_LENGTH, HeaderValue::from(encoded_len));
    // Append Vary: Accept-Encoding so caches store both compressed and
    // uncompressed variants — unless the origin already declared it (static
    // serving does, on representations whose range behavior depends on the
    // encoding).
    let already_varies = response
        .headers()
        .get_all(header::VARY)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|member| member.trim().eq_ignore_ascii_case("accept-encoding"));
    if !already_varies {
        response
            .headers_mut()
            .append(header::VARY, HeaderValue::from_static("Accept-Encoding"));
    }
}

/// Would a buffered 200 response with this MIME type and size be compressed
/// for a client that accepts a coding? Static serving uses this to disable
/// range handling for such representations: a client resuming a compressed
/// download (with or without If-Range) would otherwise receive identity bytes
/// to splice onto a compressed prefix. nginx does the same by clearing
/// `allow_ranges` in its gzip filter. Streaming bodies are never compressed
/// regardless of this answer, so the streaming path must not consult it.
pub(crate) fn would_compress(content_type: &str, size: u64) -> bool {
    is_compressible(content_type)
        && (MIN_COMPRESS_SIZE as u64..=MAX_COMPRESS_SIZE as u64).contains(&size)
}

/// The weight the client attached to `coding`, in thousandths. RFC 9110 caps a
/// qvalue at three decimals, so thousandths hold every legal value exactly and
/// spare us float comparison.
///
/// Zero means "do not send this coding": either the client wrote `;q=0`, which
/// §12.5.3 defines as "not acceptable" rather than "supported", or the coding is
/// absent from a header that carries no `*` to cover it. An empty field value
/// therefore refuses everything, which is what it means — only identity remains.
///
/// A coding named twice is read at its first weight; the field grammar does not
/// define duplicates, and no ordering is more defensible than another.
fn qvalue(accept_encoding: &str, coding: &str) -> u16 {
    let mut wildcard = None;
    for element in accept_encoding.split(',') {
        let mut parts = element.split(';');
        let name = parts.next().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        // A weight we cannot parse is not a refusal: the coding is still listed,
        // so it keeps the default weight rather than being dropped over a
        // malformed parameter.
        let weight = parts.find_map(parse_weight).unwrap_or(1000);
        if name.eq_ignore_ascii_case(coding) {
            return weight;
        }
        if name == "*" {
            wildcard = Some(weight);
        }
    }
    wildcard.unwrap_or(0)
}

/// Reads `q=<qvalue>` out of one parameter. `None` for any other parameter and
/// for a weight outside the grammar, letting the caller fall back to the default.
fn parse_weight(parameter: &str) -> Option<u16> {
    let (name, value) = parameter.split_once('=')?;
    if !name.trim().eq_ignore_ascii_case("q") {
        return None;
    }
    // qvalue = ( "0" [ "." 0*3DIGIT ] ) / ( "1" [ "." 0*3("0") ] )
    let value = value.trim();
    let (integer, fraction) = value.split_once('.').unwrap_or((value, ""));
    let mut weight: u16 = match integer {
        "0" => 0,
        "1" => 1000,
        _ => return None,
    };
    if fraction.len() > 3 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    for (digit, scale) in fraction.bytes().zip([100, 10, 1]) {
        weight += u16::from(digit - b'0') * scale;
    }
    Some(weight.min(1000))
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

/// The level to build an artifact at — one that will be cached and served many
/// times. The cost is paid once, so the only thing worth optimizing is size:
/// over real assets top-of-range brotli lands 8–12% below what the per-request
/// default produces, at a price (tens of milliseconds per file) no request
/// should ever pay directly.
pub(crate) fn artifact_level(coding: Coding) -> i32 {
    match coding {
        Coding::Br => 11,
        Coding::Gzip => 9,
        Coding::Zstd => 19,
    }
}

/// Compress bytes for the static artifact cache. `None` when the result would
/// not be smaller, same as the per-request path.
pub(crate) fn compress_artifact(data: &[u8], coding: Coding) -> Option<Vec<u8>> {
    compress(data, coding, artifact_level(coding))
}

/// Compress data with one coding. `None` when the result would not be smaller
/// than the input, which is the caller's signal to send it unencoded.
fn compress(data: &[u8], coding: Coding, level: i32) -> Option<Vec<u8>> {
    match coding {
        Coding::Br => compress_brotli(data, level),
        Coding::Gzip => compress_gzip(data, level),
        Coding::Zstd => compress_zstd(data, level),
    }
}

/// Compress data using zstd. Returns None if compression would not reduce size.
fn compress_zstd(data: &[u8], level: i32) -> Option<Vec<u8>> {
    let output = zstd::bulk::compress(data, level)
        .inspect_err(|e| tracing::debug!(error = %e, "Zstd compression failed"))
        .ok()?;
    (output.len() < data.len()).then_some(output)
}

/// Compress data using gzip. Returns None if compression would not reduce size.
fn compress_gzip(data: &[u8], level: i32) -> Option<Vec<u8>> {
    use std::io::Write;

    let mut encoder = flate2::write::GzEncoder::new(
        Vec::with_capacity(data.len() / 2),
        flate2::Compression::new(level as u32),
    );
    let output = encoder
        .write_all(data)
        .and_then(|()| encoder.finish())
        .inspect_err(|e| tracing::debug!(error = %e, "Gzip compression failed"))
        .ok()?;
    (output.len() < data.len()).then_some(output)
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

    /// The question every `Accept-Encoding` test below asks, before the choice
    /// between codings enters into it.
    fn accepts_brotli(accept_encoding: &str) -> bool {
        qvalue(accept_encoding, "br") > 0
    }

    fn levels(brotli: i32, gzip: i32, zstd: i32) -> Levels {
        Levels { brotli, gzip, zstd }
    }

    /// Everything on, at the shipped defaults.
    fn all() -> Levels {
        levels(5, 6, 6)
    }

    fn per_request(accept_encoding: &str, levels: Levels) -> Option<Coding> {
        Acceptable::parse(accept_encoding, levels).per_request()
    }

    fn artifact(accept_encoding: &str, levels: Levels) -> Option<Coding> {
        Acceptable::parse(accept_encoding, levels).artifact()
    }

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
    fn test_zero_qvalue_is_a_refusal() {
        // RFC 9110 §12.5.3: "a qvalue of 0 means 'not acceptable'".
        assert!(!accepts_brotli("br;q=0"));
        assert!(!accepts_brotli("br;q=0.0"));
        assert!(!accepts_brotli("br;q=0.000"));
        assert!(!accepts_brotli("gzip, br;q=0"));
        // A refusal of one coding says nothing about another.
        assert!(accepts_brotli("gzip;q=0, br"));
        // The smallest non-zero weight still accepts.
        assert!(accepts_brotli("br;q=0.001"));
    }

    #[test]
    fn test_wildcard() {
        // "*" covers codings the header does not name.
        assert!(accepts_brotli("*"));
        assert!(accepts_brotli("gzip, *"));
        assert!(!accepts_brotli("*;q=0"));
        // An explicit entry wins over the wildcard, in both directions.
        assert!(!accepts_brotli("*, br;q=0"));
        assert!(accepts_brotli("*;q=0, br"));
    }

    #[test]
    fn test_syntax_tolerance() {
        // Coding names are case-insensitive, and OWS is allowed around the
        // separators.
        assert!(accepts_brotli("BR"));
        assert!(accepts_brotli("gzip , BR ; q=0.9"));
        assert!(accepts_brotli("br;Q=1"));
        assert!(!accepts_brotli("BR;Q=0"));
        // Parameters other than q are ignored.
        assert!(accepts_brotli("br;foo=bar"));
        assert!(!accepts_brotli("br;foo=bar;q=0"));
        // A malformed weight is not a refusal — the coding stays listed.
        assert!(accepts_brotli("br;q=nonsense"));
        assert!(accepts_brotli("br;q="));
        // Empty elements are skipped rather than swallowing the list.
        assert!(accepts_brotli(", ,br"));
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

        // An event stream is excluded by type, not only by the shape of its
        // body: a script that sets the header and never flushes produces a
        // buffered response that would otherwise qualify. Widening this list
        // to a `text/*` prefix would start compressing Server-Sent Events,
        // which browsers accept and intermediaries then hold onto.
        assert!(!is_compressible("text/event-stream"));
        assert!(!is_compressible("text/event-stream; charset=utf-8"));
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

        let result = maybe_compress(response, Coding::Br, 4).await;

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

    /// The level brotli ships at sits above `blocking_threshold`'s steepness
    /// line, so a page-sized body is compressed on the blocking pool rather
    /// than inline. Same bytes out, different thread — and this is the path
    /// most brotli responses now take.
    #[tokio::test]
    async fn a_page_sized_brotli_body_survives_the_blocking_pool() {
        let body = "body { color: rebeccapurple; margin: 0 }\n".repeat(200);
        assert!(body.len() > blocking_threshold(Coding::Br, 5));
        let response = build_response("text/css", body.as_bytes());

        let result = maybe_compress(response, Coding::Br, 5).await;

        assert_eq!(
            result.headers().get(header::CONTENT_ENCODING).unwrap(),
            "br"
        );
        let compressed = result.into_body().collect().await.unwrap().to_bytes();
        let mut decoded = Vec::new();
        brotli::BrotliDecompress(&mut compressed.as_ref(), &mut decoded).unwrap();
        assert_eq!(decoded, body.as_bytes());
    }

    #[tokio::test]
    async fn test_skip_non_compressible_type() {
        let body = vec![0u8; 500];
        let response = build_response("image/png", &body);

        let result = maybe_compress(response, Coding::Br, 4).await;

        assert!(result.headers().get(header::CONTENT_ENCODING).is_none());
    }

    #[tokio::test]
    async fn test_skip_small_body() {
        let response = build_response("text/html", b"<h1>Hi</h1>");

        let result = maybe_compress(response, Coding::Br, 4).await;

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

        let result = maybe_compress(response, Coding::Br, 4).await;

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

        let result = maybe_compress(response, Coding::Br, 4).await;

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
            maybe_compress(response, Coding::Br, 4),
        )
        .await;

        assert!(
            result.is_ok(),
            "maybe_compress buffered a live stream instead of passing it through"
        );
        drop(tx);
    }

    #[tokio::test]
    async fn test_compress_does_not_duplicate_vary() {
        // Static serving already declares Vary: Accept-Encoding on
        // compression-eligible representations — encoding one must not
        // append a second copy.
        let body = "a".repeat(500);
        let response = Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, "text/html")
            .header(header::VARY, "Accept-Encoding")
            .body(full_body(Bytes::from(body)))
            .unwrap();

        let result = maybe_compress(response, Coding::Br, 4).await;

        assert_eq!(
            result.headers().get(header::CONTENT_ENCODING).unwrap(),
            "br"
        );
        let accept_encoding_members = result
            .headers()
            .get_all(header::VARY)
            .iter()
            .flat_map(|v| v.to_str().unwrap().split(','))
            .filter(|m| m.trim().eq_ignore_ascii_case("accept-encoding"))
            .count();
        assert_eq!(accept_encoding_members, 1);
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

        let result = maybe_compress(response, Coding::Br, 4).await;

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

        let result = maybe_compress(response, Coding::Br, 4).await;

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

        let result = maybe_compress(response, Coding::Br, 4).await;

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

        let result = maybe_compress(response, Coding::Br, 4).await;

        assert!(result.headers().get(header::CONTENT_ENCODING).is_none());
    }

    #[test]
    fn test_a_tie_is_broken_by_what_happens_to_the_bytes() {
        // What every browser sends: several codings, no weights at all. The
        // same header resolves differently depending on whether the result is
        // compressed for this request or kept and served again.
        let header = "gzip, deflate, br, zstd";
        assert_eq!(per_request(header, all()), Some(Coding::Zstd));
        assert_eq!(artifact(header, all()), Some(Coding::Br));
        // Explicit equal weights are still a tie.
        assert_eq!(
            per_request("br;q=0.5, zstd;q=0.5", all()),
            Some(Coding::Zstd)
        );
        assert_eq!(artifact("br;q=0.5, zstd;q=0.5", all()), Some(Coding::Br));
    }

    #[test]
    fn test_a_client_that_offers_one_coding_gets_that_coding() {
        // Neither preference order can be honored, so both agree.
        for coding in Coding::ALL {
            assert_eq!(per_request(coding.name(), all()), Some(coding));
            assert_eq!(artifact(coding.name(), all()), Some(coding));
        }
    }

    #[test]
    fn test_client_weights_outrank_server_preference() {
        // A client that ranks gzip above the rest is asking for gzip, and
        // §12.5.3 makes that ranking binding on the server.
        assert_eq!(per_request("br, zstd, gzip;q=1", all()), Some(Coding::Zstd));
        assert_eq!(
            per_request("br;q=0.5, zstd;q=0.5, gzip", all()),
            Some(Coding::Gzip)
        );
        assert_eq!(
            artifact("br;q=0.5, zstd;q=0.5, gzip", all()),
            Some(Coding::Gzip)
        );
        // Brotli demoted below zstd loses even on the path that prefers it.
        assert_eq!(artifact("br;q=0.5, zstd", all()), Some(Coding::Zstd));
    }

    #[test]
    fn test_negotiate_falls_back_to_gzip() {
        // The client neither brotli nor zstd reaches: Chromium over plain
        // HTTP, most command-line tools, anything older than 2016.
        assert_eq!(per_request("gzip, deflate", all()), Some(Coding::Gzip));
        assert_eq!(
            per_request("br;q=0, zstd;q=0, gzip", all()),
            Some(Coding::Gzip)
        );
        // Codings this server does not produce are not fallbacks.
        assert_eq!(per_request("deflate, compress", all()), None);
        assert_eq!(per_request("", all()), None);
    }

    #[test]
    fn test_negotiate_wildcard_takes_server_preference() {
        // "*" accepts everything, so the choice is entirely the server's.
        assert_eq!(per_request("*", all()), Some(Coding::Zstd));
        assert_eq!(artifact("*", all()), Some(Coding::Br));
        assert_eq!(per_request("*", levels(0, 6, 0)), Some(Coding::Gzip));
        assert_eq!(per_request("*", levels(0, 0, 0)), None);
        assert_eq!(per_request("*;q=0", all()), None);
    }

    #[test]
    fn test_negotiate_skips_disabled_codings() {
        // A coding left out of COMPRESSION_ENCODINGS, or set to level zero,
        // arrives here as a zero level and is not offered to anyone.
        assert_eq!(
            per_request("gzip, br, zstd", levels(4, 0, 0)),
            Some(Coding::Br)
        );
        assert_eq!(per_request("gzip", levels(4, 0, 0)), None);
        assert_eq!(
            per_request("gzip, br, zstd", levels(0, 0, 6)),
            Some(Coding::Zstd)
        );
        // Nothing offered is the same as nothing accepted.
        assert_eq!(per_request("gzip, br, zstd", levels(0, 0, 0)), None);
        assert_eq!(artifact("gzip, br, zstd", levels(0, 0, 0)), None);
    }

    #[test]
    fn test_coding_names_parse_back_out_of_configuration() {
        for coding in Coding::ALL {
            assert_eq!(Coding::from_name(coding.name()), Some(coding));
            assert_eq!(
                Coding::from_name(&coding.name().to_uppercase()),
                Some(coding)
            );
        }
        assert_eq!(Coding::from_name("brotli"), Some(Coding::Br));
        assert_eq!(Coding::from_name("deflate"), None);
        assert_eq!(Coding::from_name(""), None);
    }

    #[test]
    fn test_compress_gzip_roundtrip() {
        use std::io::Read;

        let data = "Hello, World! ".repeat(50);
        let compressed = compress_gzip(data.as_bytes(), 6).expect("repetitive text compresses");

        let mut decompressed = Vec::new();
        flate2::read::GzDecoder::new(&compressed[..])
            .read_to_end(&mut decompressed)
            .unwrap();
        assert_eq!(decompressed, data.as_bytes());
    }

    #[tokio::test]
    async fn test_compress_gzip_response() {
        let body = "a".repeat(500);
        let response = build_response("text/html", body.as_bytes());

        let result = maybe_compress(response, Coding::Gzip, 6).await;

        assert_eq!(
            result.headers().get(header::CONTENT_ENCODING).unwrap(),
            "gzip"
        );
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

    #[test]
    fn test_every_coding_has_a_distinct_slot() {
        // The artifact cache indexes an array by `slot()`; two codings sharing
        // one would silently serve each other's bytes.
        let slots: Vec<usize> = Coding::ALL.iter().map(|c| c.slot()).collect();
        for (i, slot) in slots.iter().enumerate() {
            assert!(*slot < Coding::COUNT);
            assert!(!slots[..i].contains(slot));
        }
    }
}
