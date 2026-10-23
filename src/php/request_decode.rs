//! Decoding of query-string and cookie pairs for the request object API, and
//! of the header values PHP is handed.
//!
//! Kept out of `sapi.rs` so it builds and gets tested without linking PHP —
//! these are pure functions and the rules they encode are subtle enough to
//! want direct coverage.
//!
//! The pair decoding is byte-oriented. PHP strings are byte strings, the FFI
//! boundary is `(ptr, len)` handed to `RETURN_STRINGL`, and percent-escapes
//! can encode bytes that are not valid UTF-8 at all — `setcookie('sid',
//! random_bytes(16))` is the ordinary case, not a corner one. Decoding
//! through `String` would replace those bytes with U+FFFD and silently break
//! any signature check over the value, so the decoded forms stay `Vec<u8>`.

use std::borrow::Cow;
use std::os::raw::c_char;

use http::header::{CONTENT_LENGTH, CONTENT_TYPE, HOST};
use http::{HeaderMap, HeaderName};

/// Upper bound on query pairs materialised per request.
///
/// Bounds how far a long query string can amplify into per-request memory:
/// with a typical 8 KB request-line limit some 2000 pairs can arrive, so the
/// cap is doing work rather than decorating.
///
/// The number is PHP's `max_input_vars` default, which is where the two
/// agree — but that setting is `PHP_INI_SYSTEM|PHP_INI_PERDIR`, so a
/// deployment that raises it gets a `$_GET` with more entries than `query()`
/// reports, and PHP warns at its own limit where this simply stops. Reading
/// the configured value would need a bridge accessor and a startup-time read
/// (`set_request_data` runs before `php_request_startup()`), which is a
/// larger change than the bound it would refine.
const MAX_QUERY_PAIRS: usize = 1000;

/// Percent-decode `s` into raw bytes.
///
/// `plus_as_space` picks between the two decodings PHP itself applies when it
/// builds the superglobals (`main/php_variables.c`): query strings go through
/// `php_url_decode`, which also turns `+` into a space, while cookie values go
/// through `php_raw_url_decode`, which leaves `+` alone.
pub fn url_decode(s: &[u8], plus_as_space: bool) -> Vec<u8> {
    if plus_as_space && s.contains(&b'+') {
        let swapped: Vec<u8> = s
            .iter()
            .map(|&b| if b == b'+' { b' ' } else { b })
            .collect();
        percent_encoding::percent_decode(&swapped).collect()
    } else {
        percent_encoding::percent_decode(s).collect()
    }
}

/// Split a query string into decoded (name, value) pairs.
///
/// Splitting happens on the literal separators first, so an encoded `%26`
/// stays part of a value. A pair with no `=` yields an empty value, matching
/// PHP: `?flag` registers `$_GET['flag'] === ''`. Stops at
/// [`MAX_QUERY_PAIRS`].
///
/// Names are decoded but not otherwise transformed. PHP additionally rewrites
/// `' '`, `'.'` and `'['` in `$_GET` keys (`php_register_variable_ex`), so
/// `?a.b=1` lands in `$_GET` as `a_b` while this returns `a.b` — the name the
/// client actually sent. The decoding rules match; the key mangling
/// deliberately does not.
pub fn parse_query_pairs(query: &str, out: &mut Vec<(Vec<u8>, Vec<u8>)>) {
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        if out.len() >= MAX_QUERY_PAIRS {
            break;
        }
        let bytes = pair.as_bytes();
        match bytes.iter().position(|&b| b == b'=') {
            Some(eq) => out.push((
                url_decode(&bytes[..eq], true),
                url_decode(&bytes[eq + 1..], true),
            )),
            None => out.push((url_decode(bytes, true), Vec::new())),
        }
    }
}

/// The value PHP is handed for the request header `name`, or `None` when it
/// is absent or cannot be read as text.
///
/// hyper and h2 keep each field line of a repeated header as a value of its
/// own, and PHP is handed one value per name, as RFC 3875 §4.1.18 asks of a
/// CGI server: the lines joined with `", "` in the order they arrived — the
/// combination RFC 9110 §5.3 allows a recipient, and what PHP's built-in
/// server hands its scripts. A line [`http::HeaderValue::to_str`] refuses
/// leaves out the whole header, not just that line, so PHP never takes the
/// lines that are left for all the client sent.
///
/// `Content-Type`, `Host` and `Content-Length` hold one value only (RFC 9110
/// §8.3, §7.2, §8.6), and each is its first line, whatever later lines hold.
/// Joined, they would be a value no line carried, and one PHP acts on: it
/// could take a multipart `boundary` from a later `Content-Type` line than
/// the one it picks the body parser by, `HTTP_HOST` would not be the line
/// `SERVER_NAME` is taken from when no trusted proxy names the host, and
/// `"5, 7"` is not a length. Any other field that holds one value is joined
/// like a list, so a reader sees more than one member in it (RFC 9110 §5.5)
/// rather than one of its lines passing for all of them.
///
/// h2 reads the body by the first `Content-Length` line and keeps any further
/// one in the map. Over HTTP/1.1 there is at most one: hyper drops every line
/// that follows `Transfer-Encoding` and, of those before it, keeps the first,
/// dropping a repeat that agrees with it and refusing one that disagrees.
pub fn header_field_value<'a>(headers: &'a HeaderMap, name: &HeaderName) -> Option<Cow<'a, str>> {
    let mut lines = headers.get_all(name).iter();
    let mut value = Cow::Borrowed(lines.next()?.to_str().ok()?);
    if name == CONTENT_TYPE || name == HOST || name == CONTENT_LENGTH {
        return Some(value);
    }
    for line in lines {
        let joined = value.to_mut();
        joined.push_str(", ");
        joined.push_str(line.to_str().ok()?);
    }
    Some(value)
}

/// Borrowed `(ptr, len)` for a value handed back to C.
///
/// An empty `Vec` has a dangling data pointer; the C side is handed a static
/// empty string instead, so a zero length always comes with a readable
/// pointer. Whether a zero-length value then reaches PHP as `''` or as `null`
/// is the caller's business — `oxphp_req_query_param` keeps it, while
/// `oxphp_req_cookie` historically dropped it.
pub fn value_ptr(v: &[u8]) -> (*const c_char, usize) {
    if v.is_empty() {
        (c"".as_ptr(), 0)
    } else {
        (v.as_ptr() as *const c_char, v.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(query: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
        let mut out = Vec::new();
        parse_query_pairs(query, &mut out);
        out
    }

    /// Readable comparison helper for the ASCII/UTF-8 cases.
    fn str_pairs(query: &str) -> Vec<(String, String)> {
        pairs(query)
            .into_iter()
            .map(|(k, v)| (String::from_utf8(k).unwrap(), String::from_utf8(v).unwrap()))
            .collect()
    }

    #[test]
    fn url_decode_query_style_decodes_percent_and_plus() {
        assert_eq!(
            url_decode(b"%D0%9F%D1%80%D0%B8%D0%B2%D0%B5%D1%82", true),
            "Привет".as_bytes()
        );
        assert_eq!(url_decode(b"a+b", true), b"a b");
        assert_eq!(url_decode(b"a%20b", true), b"a b");
        assert_eq!(url_decode(b"a%26b", true), b"a&b");
    }

    #[test]
    fn url_decode_cookie_style_keeps_plus_literal() {
        // php_raw_url_decode: percent-escapes yes, '+' no.
        assert_eq!(url_decode(b"a+b", false), b"a+b");
        assert_eq!(url_decode(b"a%20b", false), b"a b");
        assert_eq!(url_decode(b"%D0%9F", false), "П".as_bytes());
    }

    #[test]
    fn url_decode_preserves_non_utf8_bytes() {
        // The whole reason the decoded forms are bytes: a signed cookie or a
        // binary session id must survive verbatim, not become U+FFFD.
        assert_eq!(url_decode(b"%FF", true), vec![0xFF]);
        assert_eq!(url_decode(b"%00%FF%FE", false), vec![0x00, 0xFF, 0xFE]);
        let raw: Vec<u8> = (0u8..=255).collect();
        let encoded: String = raw.iter().map(|b| format!("%{b:02X}")).collect();
        assert_eq!(url_decode(encoded.as_bytes(), false), raw);
    }

    #[test]
    fn url_decode_passes_through_undecodable_input() {
        // A lone '%' is not an escape; PHP leaves it as-is and so do we.
        assert_eq!(url_decode(b"100%", true), b"100%");
        assert_eq!(url_decode(b"%zz", true), b"%zz");
    }

    #[test]
    fn parse_query_pairs_decodes_keys_and_values() {
        assert_eq!(
            str_pairs("%D0%BA%D0%BB%D1%8E%D1%87=%D0%B7%D0%BD%D0%B0%D1%87"),
            vec![("ключ".to_string(), "знач".to_string())]
        );
    }

    #[test]
    fn parse_query_pairs_splits_before_decoding() {
        // %26 must survive as data: splitting happens on literal '&' first.
        assert_eq!(
            str_pairs("a=1%262&b=3"),
            vec![
                ("a".to_string(), "1&2".to_string()),
                ("b".to_string(), "3".to_string()),
            ]
        );
    }

    #[test]
    fn parse_query_pairs_keeps_non_utf8_values() {
        assert_eq!(
            pairs("sid=%FF%00"),
            vec![(b"sid".to_vec(), vec![0xFF, 0x00])]
        );
    }

    #[test]
    fn parse_query_pairs_handles_valueless_and_empty_entries() {
        assert_eq!(str_pairs("flag"), vec![("flag".to_string(), String::new())]);
        assert_eq!(str_pairs("k="), vec![("k".to_string(), String::new())]);
        assert!(pairs("").is_empty());
        // Stray separators produce no phantom entries.
        assert_eq!(
            str_pairs("&a=1&&"),
            vec![("a".to_string(), "1".to_string())]
        );
    }

    #[test]
    fn parse_query_pairs_keeps_duplicate_keys() {
        assert_eq!(
            str_pairs("a=1&a=2"),
            vec![
                ("a".to_string(), "1".to_string()),
                ("a".to_string(), "2".to_string()),
            ]
        );
    }

    #[test]
    fn parse_query_pairs_does_not_mangle_names_the_way_php_does() {
        // Deliberate divergence: $_GET would key these as `a_b`.
        assert_eq!(
            str_pairs("a.b=1"),
            vec![("a.b".to_string(), "1".to_string())]
        );
        assert_eq!(
            str_pairs("a+b=1"),
            vec![("a b".to_string(), "1".to_string())]
        );
    }

    #[test]
    fn parse_query_pairs_stops_at_the_cap() {
        let query: String = (0..MAX_QUERY_PAIRS + 50)
            .map(|i| format!("k{i}=1"))
            .collect::<Vec<_>>()
            .join("&");
        assert_eq!(pairs(&query).len(), MAX_QUERY_PAIRS);
    }

    fn header_map(lines: &[(&'static str, &[u8])]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for &(name, value) in lines {
            headers.append(name, http::HeaderValue::from_bytes(value).unwrap());
        }
        headers
    }

    fn field_value<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<Cow<'a, str>> {
        header_field_value(headers, &HeaderName::from_static(name))
    }

    #[test]
    fn header_field_value_borrows_a_single_line() {
        let headers = header_map(&[("x-foo", b"one")]);
        assert!(matches!(
            field_value(&headers, "x-foo"),
            Some(Cow::Borrowed("one"))
        ));
        assert_eq!(field_value(&headers, "x-absent"), None);
    }

    #[test]
    fn header_field_value_joins_repeated_lines_in_arrival_order() {
        let headers = header_map(&[
            ("x-foo", b"one"),
            ("x-bar", b"other"),
            ("x-foo", b"two"),
            ("x-foo", b"three"),
        ]);
        assert_eq!(
            field_value(&headers, "x-foo").as_deref(),
            Some("one, two, three")
        );
        // An empty line still holds its place in the list.
        let headers = header_map(&[("x-foo", b""), ("x-foo", b"two")]);
        assert_eq!(field_value(&headers, "x-foo").as_deref(), Some(", two"));
    }

    #[test]
    fn header_field_value_leaves_out_a_header_with_any_unreadable_line() {
        for lines in [
            [("x-bin", b"\x80".as_slice()), ("x-bin", b"ok".as_slice())],
            [("x-bin", b"ok".as_slice()), ("x-bin", b"\xff".as_slice())],
        ] {
            assert_eq!(field_value(&header_map(&lines), "x-bin"), None);
        }
        assert_eq!(
            field_value(&header_map(&[("x-bin", b"\x80")]), "x-bin"),
            None
        );
    }

    #[test]
    fn header_field_value_keeps_the_first_line_of_a_one_value_field() {
        for name in ["content-type", "host", "content-length"] {
            for later in [&b"second"[..], b"\x80"] {
                let headers = header_map(&[(name, b"first"), (name, later)]);
                assert!(
                    matches!(field_value(&headers, name), Some(Cow::Borrowed("first"))),
                    "{name}"
                );
            }
        }
    }

    #[test]
    fn value_ptr_gives_a_readable_pointer_for_empty_values() {
        let (ptr, len) = value_ptr(b"");
        assert_eq!(len, 0);
        assert!(!ptr.is_null());
        // Must be dereferenceable: an empty Vec's own pointer is dangling.
        assert_eq!(unsafe { *ptr }, 0);

        let (ptr, len) = value_ptr(b"abc");
        assert_eq!(len, 3);
        assert_eq!(
            unsafe { std::slice::from_raw_parts(ptr as *const u8, len) },
            b"abc"
        );
    }
}
