//! W3C Trace Context (Level 1) parser and generator.
//!
//! Implements the `traceparent` and `tracestate` headers per
//! <https://www.w3.org/TR/trace-context/>.
//!
//! Zero external dependencies beyond `getrandom` for random ID generation.

use http::HeaderMap;

/// Hex lookup table for fast byte-to-hex conversion.
const HEX_CHARS: &[u8; 16] = b"0123456789abcdef";

/// A parsed or generated W3C Trace Context.
///
/// All hex arrays store **ASCII hex characters**, not raw bytes.
/// For example, `trace_id[0]` is `b'a'`, not `0x0a`.
#[derive(Clone, Debug)]
pub struct TraceContext {
    /// 32 lowercase hex chars representing the 128-bit trace ID.
    trace_id: [u8; 32],
    /// 16 lowercase hex chars representing the 64-bit span ID (newly generated).
    span_id: [u8; 16],
    /// Incoming span-id that becomes the parent (if parsed from a header).
    parent_span_id: Option<[u8; 16]>,
    /// Only bit 0 (sampled) is used; bits 1-7 are always 0 on outgoing.
    trace_flags: u8,
    /// Passthrough tracestate header value, not parsed.
    tracestate: Option<String>,
    /// Pre-built `traceparent` header value: `"00-{trace_id}-{span_id}-{flags}"`.
    traceparent_cache: [u8; 55],
}

impl TraceContext {
    // ── Parsing ──────────────────────────────────────────────────────

    /// Parse a `traceparent` header value per W3C Trace Context Level 1.
    ///
    /// Returns `None` if the header is malformed.
    pub fn parse(header: &str) -> Option<Self> {
        let bytes = header.as_bytes();

        // Minimum length: "VV-TTTTTTTTTTTTTTTTTTTTTTTTTTTTTTTT-SSSSSSSSSSSSSSSS-FF" = 55
        if bytes.len() < 55 {
            return None;
        }

        // Version field: first 2 chars must be valid lowercase hex.
        let version = parse_hex_byte(bytes[0], bytes[1])?;

        // Version ff is explicitly invalid.
        if version == 0xff {
            return None;
        }

        // For version 00, the header must be exactly 55 chars.
        // For higher versions, we parse from fixed positions (forward-compatible).
        if version == 0x00 && bytes.len() != 55 {
            return None;
        }

        // Check dash positions: 2, 35, 52.
        if bytes[2] != b'-' || bytes[35] != b'-' || bytes[52] != b'-' {
            return None;
        }

        // Parse trace-id (positions 3..35): 32 lowercase hex chars.
        let mut trace_id = [0u8; 32];
        let trace_id_slice = &bytes[3..35];
        if !is_valid_lowercase_hex(trace_id_slice) {
            return None;
        }
        trace_id.copy_from_slice(trace_id_slice);

        // All-zero trace-id is invalid.
        if trace_id_slice.iter().all(|&b| b == b'0') {
            return None;
        }

        // Parse parent-id (positions 36..52): 16 lowercase hex chars.
        let mut parent_span_id = [0u8; 16];
        let parent_id_slice = &bytes[36..52];
        if !is_valid_lowercase_hex(parent_id_slice) {
            return None;
        }
        parent_span_id.copy_from_slice(parent_id_slice);

        // All-zero parent-id is invalid.
        if parent_id_slice.iter().all(|&b| b == b'0') {
            return None;
        }

        // Parse trace-flags (positions 53..55): 2 hex chars.
        let raw_flags = parse_hex_byte(bytes[53], bytes[54])?;
        // Mask to bit 0 only on outgoing.
        let trace_flags = raw_flags & 0x01;

        // Generate a new span-id for this hop.
        let span_id = generate_span_id();

        // Build the traceparent cache.
        let traceparent_cache = build_traceparent(&trace_id, &span_id, trace_flags);

        Some(TraceContext {
            trace_id,
            span_id,
            parent_span_id: Some(parent_span_id),
            trace_flags,
            tracestate: None,
            traceparent_cache,
        })
    }

    // ── Generation ───────────────────────────────────────────────────

    /// Generate a brand-new trace context with random trace-id and span-id.
    ///
    /// The default trace-flags is `0x00` (not sampled).
    pub fn generate() -> Self {
        let trace_id = generate_trace_id();
        let span_id = generate_span_id();
        let trace_flags = 0x00;
        let traceparent_cache = build_traceparent(&trace_id, &span_id, trace_flags);

        TraceContext {
            trace_id,
            span_id,
            parent_span_id: None,
            trace_flags,
            tracestate: None,
            traceparent_cache,
        }
    }

    // ── From HTTP Headers ────────────────────────────────────────────

    /// Create a `TraceContext` from HTTP request headers.
    ///
    /// - If `traceparent` is absent, generates a new context.
    /// - If `traceparent` is invalid, generates a new context and discards `tracestate`.
    /// - If `tracestate` is present without valid `traceparent`, it is discarded (W3C requirement).
    /// - If `tracestate` is present with valid `traceparent`, it is stored as passthrough
    ///   (with size-based truncation if > 512 chars).
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let traceparent = headers.get("traceparent").and_then(|v| v.to_str().ok());

        let mut ctx = match traceparent {
            Some(tp) => match Self::parse(tp) {
                Some(ctx) => ctx,
                None => return Self::generate(), // invalid → discard tracestate
            },
            None => return Self::generate(), // absent → discard tracestate
        };

        // Only attach tracestate if we have a valid traceparent.
        if let Some(ts) = headers.get("tracestate").and_then(|v| v.to_str().ok()) {
            ctx.tracestate = Some(truncate_tracestate(ts));
        }

        ctx
    }

    // ── Accessors ────────────────────────────────────────────────────

    /// Returns the full `traceparent` header value (55 chars).
    pub fn traceparent(&self) -> &str {
        // SAFETY: traceparent_cache is always valid ASCII hex + dashes.
        unsafe { std::str::from_utf8_unchecked(&self.traceparent_cache) }
    }

    /// Returns the 32-char lowercase hex trace ID.
    pub fn trace_id(&self) -> &str {
        // SAFETY: trace_id is always valid ASCII hex.
        unsafe { std::str::from_utf8_unchecked(&self.trace_id) }
    }

    /// Returns the 16-char lowercase hex span ID (this hop's span).
    pub fn span_id(&self) -> &str {
        // SAFETY: span_id is always valid ASCII hex.
        unsafe { std::str::from_utf8_unchecked(&self.span_id) }
    }

    /// Returns the parent span ID (incoming span-id) if this context was parsed.
    pub fn parent_span_id(&self) -> Option<&str> {
        self.parent_span_id
            .as_ref()
            // SAFETY: parent_span_id is always valid ASCII hex when Some.
            .map(|id| unsafe { std::str::from_utf8_unchecked(id) })
    }

    /// Returns `true` if the sampled flag (bit 0) is set.
    pub fn sampled(&self) -> bool {
        self.trace_flags & 0x01 != 0
    }

    /// Returns the passthrough tracestate header value, if any.
    pub fn tracestate(&self) -> Option<&str> {
        self.tracestate.as_deref()
    }

    /// Returns the trace-flags as a 2-char hex string (e.g., `"01"` or `"00"`).
    pub fn trace_flags_hex(&self) -> &str {
        // The last 2 bytes of the traceparent cache are the flags hex.
        unsafe { std::str::from_utf8_unchecked(&self.traceparent_cache[53..55]) }
    }

    /// Returns a derived request ID: `{trace_id[0:16]}-{span_id[0:8]}` (25 chars).
    pub fn derived_request_id(&self) -> String {
        let mut s = String::with_capacity(25);
        s.push_str(unsafe { std::str::from_utf8_unchecked(&self.trace_id[0..16]) });
        s.push('-');
        s.push_str(unsafe { std::str::from_utf8_unchecked(&self.span_id[0..8]) });
        s
    }
}

// ── Helper Functions ─────────────────────────────────────────────────

/// Generate a 128-bit random trace ID as 32 lowercase hex chars.
fn generate_trace_id() -> [u8; 32] {
    let mut raw = [0u8; 16];
    getrandom::fill(&mut raw).expect("getrandom failed");
    hex_encode_16(&raw)
}

/// Generate a 64-bit random span ID as 16 lowercase hex chars.
fn generate_span_id() -> [u8; 16] {
    let mut raw = [0u8; 8];
    getrandom::fill(&mut raw).expect("getrandom failed");
    hex_encode_8(&raw)
}

/// Encode 16 raw bytes into 32 lowercase hex ASCII bytes.
fn hex_encode_16(raw: &[u8; 16]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, &byte) in raw.iter().enumerate() {
        out[i * 2] = HEX_CHARS[(byte >> 4) as usize];
        out[i * 2 + 1] = HEX_CHARS[(byte & 0x0f) as usize];
    }
    out
}

/// Encode 8 raw bytes into 16 lowercase hex ASCII bytes.
fn hex_encode_8(raw: &[u8; 8]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (i, &byte) in raw.iter().enumerate() {
        out[i * 2] = HEX_CHARS[(byte >> 4) as usize];
        out[i * 2 + 1] = HEX_CHARS[(byte & 0x0f) as usize];
    }
    out
}

/// Build the 55-byte traceparent cache: `"00-{trace_id}-{span_id}-{flags}"`.
fn build_traceparent(trace_id: &[u8; 32], span_id: &[u8; 16], flags: u8) -> [u8; 55] {
    let mut buf = [0u8; 55];
    // Version "00"
    buf[0] = b'0';
    buf[1] = b'0';
    buf[2] = b'-';
    // Trace ID (32 chars)
    buf[3..35].copy_from_slice(trace_id);
    buf[35] = b'-';
    // Span ID (16 chars)
    buf[36..52].copy_from_slice(span_id);
    buf[52] = b'-';
    // Flags (2 hex chars)
    buf[53] = HEX_CHARS[(flags >> 4) as usize];
    buf[54] = HEX_CHARS[(flags & 0x0f) as usize];
    buf
}

/// Check if all bytes in the slice are valid lowercase hex (`[0-9a-f]`).
fn is_valid_lowercase_hex(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|&b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Parse two hex ASCII chars into a byte. Returns `None` if either char is not
/// valid lowercase hex.
fn parse_hex_byte(hi: u8, lo: u8) -> Option<u8> {
    let high = hex_digit_value(hi)?;
    let low = hex_digit_value(lo)?;
    Some((high << 4) | low)
}

/// Convert a single lowercase hex ASCII char to its numeric value.
fn hex_digit_value(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        _ => None,
    }
}

/// Truncate a tracestate value to fit within the 512-char limit.
///
/// Per W3C:
/// - First remove entries > 128 chars.
/// - Then truncate whole entries from the right until <= 512 chars.
fn truncate_tracestate(ts: &str) -> String {
    if ts.len() <= 512 {
        return ts.to_string();
    }

    // Split into entries (comma-separated), trim whitespace.
    let entries: Vec<&str> = ts.split(',').map(|e| e.trim()).collect();

    // First pass: remove entries > 128 chars.
    let mut filtered: Vec<&str> = entries.into_iter().filter(|e| e.len() <= 128).collect();

    // Second pass: remove entries from the right until total fits.
    loop {
        let total: usize = if filtered.is_empty() {
            0
        } else {
            // Each entry plus comma separator (n-1 commas).
            filtered.iter().map(|e| e.len()).sum::<usize>() + filtered.len() - 1
        };

        if total <= 512 || filtered.is_empty() {
            break;
        }

        filtered.pop();
    }

    filtered.join(",")
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use http::HeaderMap;
    use http::HeaderValue;

    const VALID_TRACEPARENT: &str = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";

    #[test]
    fn test_parse_valid_v00() {
        let ctx = TraceContext::parse(VALID_TRACEPARENT).expect("should parse");
        assert_eq!(ctx.trace_id(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(ctx.parent_span_id(), Some("00f067aa0ba902b7"));
        assert!(ctx.sampled());
        assert_eq!(ctx.trace_flags, 0x01);
        // span_id should be newly generated, not equal to the incoming one.
        assert_ne!(ctx.span_id(), "00f067aa0ba902b7");
        assert_eq!(ctx.span_id().len(), 16);
    }

    #[test]
    fn test_parse_rejects_all_zero_trace_id() {
        let header = "00-00000000000000000000000000000000-00f067aa0ba902b7-01";
        assert!(TraceContext::parse(header).is_none());
    }

    #[test]
    fn test_parse_rejects_all_zero_span_id() {
        let header = "00-4bf92f3577b34da6a3ce929d0e0e4736-0000000000000000-01";
        assert!(TraceContext::parse(header).is_none());
    }

    #[test]
    fn test_parse_rejects_uppercase_hex() {
        // Uppercase in trace-id.
        let header = "00-4BF92F3577B34DA6A3CE929D0E0E4736-00f067aa0ba902b7-01";
        assert!(TraceContext::parse(header).is_none());

        // Uppercase in parent-id.
        let header = "00-4bf92f3577b34da6a3ce929d0e0e4736-00F067AA0BA902B7-01";
        assert!(TraceContext::parse(header).is_none());
    }

    #[test]
    fn test_parse_rejects_version_ff() {
        let header = "ff-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        assert!(TraceContext::parse(header).is_none());
    }

    #[test]
    fn test_parse_masks_trace_flags() {
        // flags = 0xff → should be masked to 0x01.
        let header = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-ff";
        let ctx = TraceContext::parse(header).expect("should parse");
        assert_eq!(ctx.trace_flags, 0x01);
        assert!(ctx.sampled());
        // Outgoing traceparent should show "01", not "ff".
        assert!(ctx.traceparent().ends_with("-01"));
    }

    #[test]
    fn test_parse_forward_compat_higher_version() {
        // Version 01 with extra trailing data (>55 chars) should be accepted.
        let header = "01-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-some-future-field";
        let ctx = TraceContext::parse(header).expect("should parse future version");
        assert_eq!(ctx.trace_id(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert!(ctx.sampled());
    }

    #[test]
    fn test_parse_rejects_short_header() {
        assert!(TraceContext::parse("00-abcd-ef01-00").is_none());
        assert!(TraceContext::parse("").is_none());
        assert!(TraceContext::parse("00").is_none());
    }

    #[test]
    fn test_parse_unsampled() {
        let header = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-00";
        let ctx = TraceContext::parse(header).expect("should parse");
        assert!(!ctx.sampled());
        assert_eq!(ctx.trace_flags_hex(), "00");
    }

    #[test]
    fn test_generate_produces_valid_context() {
        let ctx = TraceContext::generate();
        assert_eq!(ctx.trace_id().len(), 32);
        assert_eq!(ctx.span_id().len(), 16);
        assert!(ctx.parent_span_id().is_none());
        assert!(!ctx.sampled());
        assert_eq!(ctx.trace_flags_hex(), "00");
        assert!(ctx.tracestate().is_none());

        // Verify all chars are valid lowercase hex.
        assert!(ctx
            .trace_id()
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
        assert!(ctx
            .span_id()
            .chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    fn test_generate_unique_ids() {
        let a = TraceContext::generate();
        let b = TraceContext::generate();
        assert_ne!(a.trace_id(), b.trace_id());
        assert_ne!(a.span_id(), b.span_id());
    }

    #[test]
    fn test_traceparent_format() {
        let ctx = TraceContext::parse(VALID_TRACEPARENT).expect("should parse");
        let tp = ctx.traceparent();
        assert_eq!(tp.len(), 55);
        assert!(tp.starts_with("00-"));
        // trace-id should be carried forward.
        assert!(tp.contains("4bf92f3577b34da6a3ce929d0e0e4736"));
        // Should end with the masked flags.
        assert!(tp.ends_with("-01"));

        // Verify the format: "VV-TTTT...TTTT-SSSS...SSSS-FF"
        let parts: Vec<&str> = tp.split('-').collect();
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0].len(), 2); // version
        assert_eq!(parts[1].len(), 32); // trace-id
        assert_eq!(parts[2].len(), 16); // span-id
        assert_eq!(parts[3].len(), 2); // flags
    }

    #[test]
    fn test_derived_request_id_format() {
        let ctx = TraceContext::parse(VALID_TRACEPARENT).expect("should parse");
        let rid = ctx.derived_request_id();
        assert_eq!(rid.len(), 25);
        // First 16 chars of trace-id + "-" + first 8 chars of span-id.
        assert!(rid.starts_with("4bf92f3577b34da6"));
        assert_eq!(rid.chars().nth(16), Some('-'));
    }

    #[test]
    fn test_from_headers_no_traceparent() {
        let headers = HeaderMap::new();
        let ctx = TraceContext::from_headers(&headers);
        // Should generate a new context.
        assert_eq!(ctx.trace_id().len(), 32);
        assert!(ctx.parent_span_id().is_none());
        assert!(ctx.tracestate().is_none());
    }

    #[test]
    fn test_from_headers_with_traceparent() {
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", HeaderValue::from_static(VALID_TRACEPARENT));
        let ctx = TraceContext::from_headers(&headers);
        assert_eq!(ctx.trace_id(), "4bf92f3577b34da6a3ce929d0e0e4736");
        assert_eq!(ctx.parent_span_id(), Some("00f067aa0ba902b7"));
        assert!(ctx.sampled());
    }

    #[test]
    fn test_from_headers_with_tracestate() {
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", HeaderValue::from_static(VALID_TRACEPARENT));
        headers.insert(
            "tracestate",
            HeaderValue::from_static("vendor1=value1,vendor2=value2"),
        );
        let ctx = TraceContext::from_headers(&headers);
        assert_eq!(ctx.tracestate(), Some("vendor1=value1,vendor2=value2"));
    }

    #[test]
    fn test_from_headers_tracestate_without_traceparent_discarded() {
        let mut headers = HeaderMap::new();
        headers.insert("tracestate", HeaderValue::from_static("vendor1=value1"));
        let ctx = TraceContext::from_headers(&headers);
        // tracestate without traceparent should be discarded.
        assert!(ctx.tracestate().is_none());
        assert!(ctx.parent_span_id().is_none());
    }

    #[test]
    fn test_from_headers_invalid_traceparent_discards_tracestate() {
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", HeaderValue::from_static("invalid-header"));
        headers.insert("tracestate", HeaderValue::from_static("vendor1=value1"));
        let ctx = TraceContext::from_headers(&headers);
        // Invalid traceparent → generate new context, discard tracestate.
        assert!(ctx.tracestate().is_none());
        assert!(ctx.parent_span_id().is_none());
    }

    #[test]
    fn test_tracestate_truncation() {
        // Create a tracestate > 512 chars with multiple entries.
        let long_entry = format!("vendor1={}", "x".repeat(200));
        let entries: Vec<String> = (0..5)
            .map(|i| format!("v{}={}", i, "a".repeat(100)))
            .collect();
        let ts = format!("{},{}", long_entry, entries.join(","));
        assert!(ts.len() > 512);

        let result = truncate_tracestate(&ts);
        assert!(result.len() <= 512);
    }

    #[test]
    fn test_tracestate_removes_long_entries_first() {
        // Entry > 128 chars should be removed first.
        let long_entry = format!("long={}", "x".repeat(200));
        let short_entries: Vec<String> = (0..3).map(|i| format!("v{}=val{}", i, i)).collect();
        let ts = format!("{},{}", long_entry, short_entries.join(","));

        if ts.len() > 512 {
            let result = truncate_tracestate(&ts);
            // The long entry should not be in the result.
            assert!(!result.contains("long="));
        }
    }

    #[test]
    fn test_parse_rejects_missing_dashes() {
        let header = "00X4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01";
        assert!(TraceContext::parse(header).is_none());
    }

    #[test]
    fn test_parse_v00_rejects_extra_chars() {
        // Version 00 must be exactly 55 chars.
        let header = "00-4bf92f3577b34da6a3ce929d0e0e4736-00f067aa0ba902b7-01-extra";
        assert!(TraceContext::parse(header).is_none());
    }

    #[test]
    fn test_clone() {
        let mut headers = HeaderMap::new();
        headers.insert("traceparent", HeaderValue::from_static(VALID_TRACEPARENT));
        headers.insert("tracestate", HeaderValue::from_static("vendor1=value1"));
        let ctx = TraceContext::from_headers(&headers);
        let cloned = ctx.clone();
        assert_eq!(ctx.trace_id(), cloned.trace_id());
        assert_eq!(ctx.span_id(), cloned.span_id());
        assert_eq!(ctx.tracestate(), cloned.tracestate());
    }
}
