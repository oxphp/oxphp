//! PHP 8.5 `SAPI_HEADER_DELETE_PREFIX` matcher.
//!
//! Lives outside `sapi.rs` (which is `#[cfg(feature = "php")]`) so the unit
//! tests run in `cargo test --no-default-features` — the configuration PR
//! CI uses, since the host runner has no `libphp.so`.

/// Case-insensitive starts-with against the wire-form line `"{name}: {value}"`.
/// PHP rebuilds the prefix into this form in `main/SAPI.c:617-635` of PHP-8.5.6
/// before dispatching to `header_handler`, so we reproduce the match
/// byte-for-byte.
///
/// Allocation-free: walks the prefix once across the virtual chunks
/// `[name, ": ", value]`, case-folding each byte inline. The naive
/// `format!("{n}: {v}").to_ascii_lowercase().starts_with(...)` would allocate
/// twice per stored header — measurable on responses with many `Set-Cookie`
/// lines, even though this arm itself is cold path.
pub fn header_line_starts_with_prefix_ci(name: &str, value: &str, prefix: &[u8]) -> bool {
    let chunks: [&[u8]; 3] = [name.as_bytes(), b": ", value.as_bytes()];
    let mut i = 0;
    for chunk in chunks {
        for &line_byte in chunk {
            if i == prefix.len() {
                return true;
            }
            if !prefix[i].eq_ignore_ascii_case(&line_byte) {
                return false;
            }
            i += 1;
        }
    }
    i == prefix.len()
}

pub fn delete_headers_with_prefix(headers: &mut Vec<(String, String)>, prefix: &[u8]) {
    headers.retain(|(n, v)| !header_line_starts_with_prefix_ci(n, v, prefix));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(name: &str, value: &str) -> (String, String) {
        (name.to_string(), value.to_string())
    }

    #[test]
    fn name_only_removes_all_with_that_name() {
        let mut headers = vec![
            h("Set-Cookie", "a=1"),
            h("Set-Cookie", "b=2"),
            h("X-Other", "keep"),
        ];
        delete_headers_with_prefix(&mut headers, b"Set-Cookie:");
        assert_eq!(headers, vec![h("X-Other", "keep")]);
    }

    #[test]
    fn name_with_space_separator_matches() {
        let mut headers = vec![h("Set-Cookie", "a=1"), h("X-Other", "keep")];
        delete_headers_with_prefix(&mut headers, b"Set-Cookie: ");
        assert_eq!(headers, vec![h("X-Other", "keep")]);
    }

    #[test]
    fn name_plus_value_prefix_removes_only_matching_value() {
        let mut headers = vec![
            h("Set-Cookie", "PHPSESSID=abc; Path=/"),
            h("Set-Cookie", "remember=yes"),
            h("Set-Cookie", "PHPSESSID=xyz"),
        ];
        delete_headers_with_prefix(&mut headers, b"Set-Cookie: PHPSESSID=");
        assert_eq!(headers, vec![h("Set-Cookie", "remember=yes")]);
    }

    #[test]
    fn case_insensitive_on_name() {
        let mut headers = vec![h("Set-Cookie", "a=1"), h("X-Other", "keep")];
        delete_headers_with_prefix(&mut headers, b"set-cookie:");
        assert_eq!(headers, vec![h("X-Other", "keep")]);
    }

    #[test]
    fn case_insensitive_on_value() {
        let mut headers = vec![h("Set-Cookie", "PHPSESSID=abc")];
        delete_headers_with_prefix(&mut headers, b"Set-Cookie: phpsessid=");
        assert!(headers.is_empty());
    }

    #[test]
    fn no_match_preserves_all_headers() {
        let original = vec![
            h("Set-Cookie", "a=1"),
            h("Content-Type", "text/html"),
            h("X-Custom", "v"),
        ];
        let mut headers = original.clone();
        delete_headers_with_prefix(&mut headers, b"X-Missing:");
        assert_eq!(headers, original);
    }

    #[test]
    fn prefix_longer_than_line_does_not_match() {
        let mut headers = vec![h("X", "y")];
        delete_headers_with_prefix(&mut headers, b"X: y-but-much-longer");
        assert_eq!(headers, vec![h("X", "y")]);
    }

    #[test]
    fn empty_list_is_noop() {
        let mut headers: Vec<(String, String)> = Vec::new();
        delete_headers_with_prefix(&mut headers, b"Set-Cookie:");
        assert!(headers.is_empty());
    }

    #[test]
    fn match_preserves_relative_order_of_survivors() {
        let mut headers = vec![
            h("Set-Cookie", "a=1"),
            h("X-First", "1"),
            h("Set-Cookie", "b=2"),
            h("X-Second", "2"),
            h("Set-Cookie", "c=3"),
            h("X-Third", "3"),
        ];
        delete_headers_with_prefix(&mut headers, b"Set-Cookie:");
        assert_eq!(
            headers,
            vec![h("X-First", "1"), h("X-Second", "2"), h("X-Third", "3")]
        );
    }

    #[test]
    fn exact_full_line_match_removes_that_header() {
        let mut headers = vec![h("X-Trace", "abc"), h("X-Trace", "def")];
        delete_headers_with_prefix(&mut headers, b"X-Trace: abc");
        assert_eq!(headers, vec![h("X-Trace", "def")]);
    }

    #[test]
    fn empty_prefix_matches_everything() {
        // Vacuous edge case — PHP would never dispatch this in practice
        // (header_remove() requires at least a name), but the matcher is
        // byte-correct: an empty prefix is always a prefix of any line.
        let mut headers = vec![h("X-A", "1"), h("X-B", "2")];
        delete_headers_with_prefix(&mut headers, b"");
        assert!(headers.is_empty());
    }
}
