use http::header::COOKIE;
use http::{HeaderMap, HeaderValue};

/// Parsed cookies for a specific plugin, with the prefix stripped.
pub struct PluginCookies {
    /// (stripped_key, value) pairs
    pub(crate) cookies: Vec<(String, String)>,
}

impl PluginCookies {
    /// Get a cookie value by key (prefix already stripped).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.cookies
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Join a `Cookie` header that arrived as several field lines into one.
///
/// An HTTP/2 client may split its cookies across field lines for better HPACK
/// compression, and RFC 9113 section 8.2.3 requires the server to join them
/// with `"; "` before passing them to an application; neither `h2` nor `hyper`
/// does it. Over HTTP/1.1 a user agent must not send more than one (RFC 6265
/// section 5.4), and one that does is joined the same way. Called once on
/// every request before anything reads the header, so the plugins, the
/// `__oxp_*` stripping and every superglobal see the same cookies.
pub fn join_cookie_field_lines(headers: &mut HeaderMap) {
    let mut lines = headers.get_all(COOKIE).iter();
    let (Some(first), Some(_)) = (lines.next(), lines.next()) else {
        return;
    };

    let mut joined = first.as_bytes().to_vec();
    for line in headers.get_all(COOKIE).iter().skip(1) {
        joined.extend_from_slice(b"; ");
        joined.extend_from_slice(line.as_bytes());
    }

    // Each line is already a valid field value, and so is the separator. Were
    // that ever not so, dropping the header keeps a plugin cookie on a later
    // line from reaching the application.
    replace_cookie_header(headers, HeaderValue::from_bytes(&joined).ok());
}

/// Make `value` the only `Cookie` line, or drop the header when there is no
/// value or no room for one.
///
/// `HeaderMap::insert` panics when the map has to grow and cannot, even for a
/// key it already holds (http 1.4). Losing the cookies is what keeps a plugin cookie from
/// reaching the application.
fn replace_cookie_header(headers: &mut HeaderMap, value: Option<HeaderValue>) {
    let stored = value.is_some_and(|value| headers.try_insert(COOKIE, value).is_ok());
    if !stored {
        headers.remove(COOKIE);
    }
}

/// Extract cookies belonging to a specific plugin from the Cookie header.
/// `prefix` is `"__oxp_{name}_"`. Returns cookies with the prefix stripped.
pub fn extract_plugin_cookies(headers: &HeaderMap, prefix: &str) -> PluginCookies {
    let cookie_str = match headers.get(COOKIE).and_then(|v| v.to_str().ok()) {
        Some(s) => s,
        None => return PluginCookies { cookies: vec![] },
    };

    let cookies = cookie_str
        .split(';')
        .filter_map(|pair| {
            let pair = pair.trim();
            let (name, value) = pair.split_once('=')?;
            let name = name.trim();
            let value = value.trim();
            name.strip_prefix(prefix)
                .map(|stripped| (stripped.to_string(), value.to_string()))
        })
        .collect();

    PluginCookies { cookies }
}

/// Whether PHP would register this `name=value` pair under a `__oxp_` key.
///
/// PHP skips whitespace before a cookie name (`php_default_treat_data`) and
/// registers a space, a `.` or an unmatched `[` in it as `_`
/// (`php_register_variable_ex`), in 8.4 and 8.5. A `[` that opens an array, or
/// that makes PHP discard the name by coming first, counts too: this close to
/// the reserved prefix, telling them apart is not worth it.
fn is_plugin_cookie(pair: &str) -> bool {
    const PREFIX: &[u8] = b"__oxp_";
    let name = pair.trim_start().as_bytes();
    name.len() >= PREFIX.len()
        && name.iter().zip(PREFIX).all(|(&byte, &expected)| {
            byte == expected || (expected == b'_' && matches!(byte, b' ' | b'.' | b'['))
        })
}

/// Strip `__oxp_*` cookies, and names `is_plugin_cookie` reads as one, from the
/// Cookie header before PHP sees them.
///
/// A header holding a non-ASCII byte is dropped whole: nothing that reads
/// cookies accepts one, and it must not stay in the request unstripped.
pub fn strip_plugin_cookies(parts: &mut http::request::Parts) {
    let cookie_header = match parts.headers.get(COOKIE) {
        Some(h) => h,
        None => return,
    };

    let cookie_str = match cookie_header.to_str() {
        Ok(s) => s,
        Err(_) => {
            parts.headers.remove(COOKIE);
            return;
        }
    };

    if !cookie_str.split(';').any(is_plugin_cookie) {
        return;
    }

    let filtered: Vec<&str> = cookie_str
        .split(';')
        .filter(|pair| !is_plugin_cookie(pair))
        .collect();

    if filtered.is_empty() {
        parts.headers.remove(COOKIE);
    } else {
        let new_value = filtered
            .iter()
            .map(|s| s.trim())
            .collect::<Vec<_>>()
            .join("; ");
        replace_cookie_header(&mut parts.headers, HeaderValue::from_str(&new_value).ok());
    }
}

/// Set-Cookie options for plugin cookies.
#[derive(Debug, Clone, Default)]
pub struct CookieOptions {
    pub path: Option<String>,
    pub domain: Option<String>,
    pub max_age: Option<i64>,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSite>,
}

/// SameSite cookie attribute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SameSite {
    Strict,
    Lax,
    None,
}

impl SameSite {
    pub fn as_str(&self) -> &'static str {
        match self {
            SameSite::Strict => "Strict",
            SameSite::Lax => "Lax",
            SameSite::None => "None",
        }
    }
}

/// Plugin Set-Cookie entry (key/value without prefix — prefix applied by wrapper).
pub struct PluginSetCookie {
    pub key: String,
    pub value: String,
    pub opts: CookieOptions,
}

/// Format a Set-Cookie header value with the plugin prefix applied.
/// `prefix` is `"__oxp_{name}_"`.
pub fn format_set_cookie_header(prefix: &str, cookie: &PluginSetCookie) -> String {
    let mut header = format!("{}{}={}", prefix, cookie.key, cookie.value);

    if let Some(ref path) = cookie.opts.path {
        header.push_str(&format!("; Path={path}"));
    }
    if let Some(ref domain) = cookie.opts.domain {
        header.push_str(&format!("; Domain={domain}"));
    }
    if let Some(max_age) = cookie.opts.max_age {
        header.push_str(&format!("; Max-Age={max_age}"));
    }
    if cookie.opts.secure {
        header.push_str("; Secure");
    }
    if cookie.opts.http_only {
        header.push_str("; HttpOnly");
    }
    if let Some(same_site) = cookie.opts.same_site {
        header.push_str(&format!("; SameSite={}", same_site.as_str()));
    }

    header
}

#[cfg(test)]
mod tests {
    use super::*;
    use http::header::COOKIE;

    fn make_headers(cookie: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(COOKIE, http::HeaderValue::from_str(cookie).unwrap());
        headers
    }

    fn make_parts(cookie: &str) -> http::request::Parts {
        let (mut parts, _) = http::Request::builder()
            .method(http::Method::GET)
            .uri("/test")
            .body(())
            .unwrap()
            .into_parts();
        parts
            .headers
            .insert(COOKIE, http::HeaderValue::from_str(cookie).unwrap());
        parts
    }

    // ── extract_plugin_cookies tests ──

    #[test]
    fn test_extract_plugin_cookies_basic() {
        let headers = make_headers("__oxp_auth_token=abc; session=xyz; __oxp_auth_uid=123");
        let cookies = extract_plugin_cookies(&headers, "__oxp_auth_");
        assert_eq!(cookies.get("token"), Some("abc"));
        assert_eq!(cookies.get("uid"), Some("123"));
        assert_eq!(cookies.get("session"), None); // not prefixed
    }

    #[test]
    fn test_extract_plugin_cookies_isolation() {
        let headers = make_headers("__oxp_a_x=1; __oxp_b_y=2");

        let cookies_a = extract_plugin_cookies(&headers, "__oxp_a_");
        let cookies_b = extract_plugin_cookies(&headers, "__oxp_b_");

        assert_eq!(cookies_a.get("x"), Some("1"));
        assert_eq!(cookies_a.get("y"), None); // belongs to plugin "b"
        assert_eq!(cookies_b.get("y"), Some("2"));
        assert_eq!(cookies_b.get("x"), None); // belongs to plugin "a"
    }

    #[test]
    fn test_extract_plugin_cookies_empty() {
        let headers = HeaderMap::new();
        let cookies = extract_plugin_cookies(&headers, "__oxp_test_");
        assert_eq!(cookies.get("anything"), None);
    }

    #[test]
    fn test_extract_plugin_cookies_no_match() {
        let headers = make_headers("session=abc; theme=dark");
        let cookies = extract_plugin_cookies(&headers, "__oxp_test_");
        assert_eq!(cookies.get("session"), None);
    }

    // ── join_cookie_field_lines tests ──

    fn cookie_lines(headers: &HeaderMap) -> Vec<&[u8]> {
        headers
            .get_all(COOKIE)
            .iter()
            .map(|v| v.as_bytes())
            .collect()
    }

    #[test]
    fn test_join_cookie_field_lines_joins_in_arrival_order() {
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, "a=1".parse().unwrap());
        headers.append(COOKIE, "__oxp_t_s=1; b=2".parse().unwrap());
        headers.append(COOKIE, "c=3".parse().unwrap());
        join_cookie_field_lines(&mut headers);
        assert_eq!(
            cookie_lines(&headers),
            [b"a=1; __oxp_t_s=1; b=2; c=3".as_slice()]
        );
    }

    #[test]
    fn test_join_cookie_field_lines_leaves_one_line_alone() {
        let mut headers = make_headers("a=1;b=2");
        join_cookie_field_lines(&mut headers);
        assert_eq!(cookie_lines(&headers), [b"a=1;b=2".as_slice()]);
    }

    #[test]
    fn test_join_cookie_field_lines_without_a_cookie_header() {
        let mut headers = HeaderMap::new();
        join_cookie_field_lines(&mut headers);
        assert!(headers.get(COOKIE).is_none());
    }

    #[test]
    fn test_join_cookie_field_lines_carries_non_ascii_bytes() {
        // Joined as bytes: what such a byte means is left to the readers, the
        // same as on a header that arrived as one line.
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, HeaderValue::from_bytes(b"bin=\xff").unwrap());
        headers.append(COOKIE, "a=1".parse().unwrap());
        join_cookie_field_lines(&mut headers);
        assert_eq!(cookie_lines(&headers), [b"bin=\xff; a=1".as_slice()]);
    }

    #[test]
    fn test_join_then_strip_removes_a_plugin_cookie_from_any_line() {
        let mut parts = make_parts("a=1");
        parts
            .headers
            .append(COOKIE, "__oxp_t_s=1; b=2".parse().unwrap());
        join_cookie_field_lines(&mut parts.headers);
        strip_plugin_cookies(&mut parts);
        assert_eq!(cookie_lines(&parts.headers), [b"a=1; b=2".as_slice()]);
    }

    // ── strip_plugin_cookies tests ──

    #[test]
    fn test_strip_plugin_cookies_basic() {
        let mut parts = make_parts("session=abc; __oxp_analytics_uid=x; theme=dark");
        strip_plugin_cookies(&mut parts);

        let cookie = parts.headers.get(COOKIE).unwrap().to_str().unwrap();
        assert_eq!(cookie, "session=abc; theme=dark");
        assert!(!cookie.contains("__oxp_"));
    }

    #[test]
    fn test_strip_plugin_cookies_all_removed() {
        let mut parts = make_parts("__oxp_a_x=1; __oxp_b_y=2");
        strip_plugin_cookies(&mut parts);

        assert!(parts.headers.get(COOKIE).is_none());
    }

    #[test]
    fn test_strip_plugin_cookies_none_present() {
        let mut parts = make_parts("session=abc; theme=dark");
        strip_plugin_cookies(&mut parts);

        let cookie = parts.headers.get(COOKIE).unwrap().to_str().unwrap();
        // Original preserved (quick check path — no __oxp_ found)
        assert!(cookie.contains("session=abc"));
    }

    #[test]
    fn test_strip_plugin_cookies_no_cookie_header() {
        let (mut parts, _) = http::Request::builder()
            .method(http::Method::GET)
            .uri("/test")
            .body(())
            .unwrap()
            .into_parts();
        strip_plugin_cookies(&mut parts); // should not panic
        assert!(parts.headers.get(COOKIE).is_none());
    }

    #[test]
    fn test_strip_plugin_cookies_drops_a_header_with_a_non_ascii_byte() {
        for cookie in [
            b"__oxp_t_s=1; app=keep; bin=\xff".as_slice(),
            b"app=keep; bin=\xff".as_slice(),
        ] {
            let mut parts = make_parts("placeholder=1");
            parts
                .headers
                .insert(COOKIE, HeaderValue::from_bytes(cookie).unwrap());
            strip_plugin_cookies(&mut parts);
            assert!(parts.headers.get(COOKIE).is_none(), "{cookie:?}");
        }
    }

    #[test]
    fn test_strip_plugin_cookies_removes_a_name_php_registers_under_the_prefix() {
        for name in [
            "__oxp.t_s",
            "__oxp t_s",
            "__oxp[t_s",
            "..oxp_t_s",
            "._oxp_t_s",
            "_.oxp_t_s",
            "_ oxp_t_s",
            "_[oxp_t_s",
        ] {
            let mut parts = make_parts(&format!("{name}=1; a=1"));
            strip_plugin_cookies(&mut parts);
            assert_eq!(cookie_lines(&parts.headers), [b"a=1".as_slice()], "{name}");
        }
    }

    #[test]
    fn test_strip_plugin_cookies_removes_a_name_with_a_bracket_php_would_keep() {
        for name in ["__oxp[a]", "_[oxp_t]", "[_oxp_t"] {
            let mut parts = make_parts(&format!("{name}=1; a=1"));
            strip_plugin_cookies(&mut parts);
            assert_eq!(cookie_lines(&parts.headers), [b"a=1".as_slice()], "{name}");
        }
    }

    #[test]
    fn test_strip_plugin_cookies_skips_whitespace_before_a_mangled_name() {
        let mut parts = make_parts("a=1;\t__oxp.t_s=1;  ..oxp_t_u=2");
        strip_plugin_cookies(&mut parts);
        assert_eq!(cookie_lines(&parts.headers), [b"a=1".as_slice()]);
    }

    #[test]
    fn test_strip_plugin_cookies_leaves_other_names_as_they_arrived() {
        for cookie in [
            "__oxp=1;a=1",
            "__oxp;a=1",
            "__.xp_t=1;a=1",
            "__oxpx=1;a=1",
            "_oxp_t=1;a=1",
            "x__oxp_t=1;a=1",
            "a=__oxp.t;b=2",
            "a=__oxp_t;b=2",
        ] {
            let mut parts = make_parts(cookie);
            strip_plugin_cookies(&mut parts);
            assert_eq!(cookie_lines(&parts.headers), [cookie.as_bytes()]);
        }
    }

    #[test]
    fn test_replace_cookie_header_without_a_value_drops_every_line() {
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, "a=1".parse().unwrap());
        headers.append(COOKIE, "b=2".parse().unwrap());
        replace_cookie_header(&mut headers, None);
        assert!(headers.get(COOKIE).is_none());
    }

    // ── a header map with no room left ──

    /// Adds distinct names until the map refuses one, so that any later call
    /// which reserves a slot fails, even for a key the map already holds.
    fn fill_to_capacity(headers: &mut HeaderMap) {
        for i in 0.. {
            let name = http::HeaderName::from_bytes(format!("x{i}").as_bytes()).unwrap();
            if headers
                .try_append(name, HeaderValue::from_static(""))
                .is_err()
            {
                break;
            }
        }
        assert_eq!(headers.keys_len(), headers.capacity());
    }

    #[test]
    fn test_join_cookie_field_lines_drops_the_header_on_a_full_map() {
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, "a=1".parse().unwrap());
        headers.append(COOKIE, "__oxp_t_s=1".parse().unwrap());
        fill_to_capacity(&mut headers);
        join_cookie_field_lines(&mut headers);
        assert!(headers.get(COOKIE).is_none());
    }

    #[test]
    fn test_strip_plugin_cookies_drops_the_header_on_a_full_map() {
        let mut parts = make_parts("__oxp_x=1; a=1");
        fill_to_capacity(&mut parts.headers);
        strip_plugin_cookies(&mut parts);
        assert!(parts.headers.get(COOKIE).is_none());
    }

    // ── format_set_cookie_header tests ──

    #[test]
    fn test_format_set_cookie_basic() {
        let cookie = PluginSetCookie {
            key: "token".into(),
            value: "abc123".into(),
            opts: CookieOptions::default(),
        };
        let header = format_set_cookie_header("__oxp_auth_", &cookie);
        assert_eq!(header, "__oxp_auth_token=abc123");
    }

    #[test]
    fn test_format_set_cookie_all_options() {
        let cookie = PluginSetCookie {
            key: "uid".into(),
            value: "xyz".into(),
            opts: CookieOptions {
                path: Some("/".into()),
                domain: Some(".example.com".into()),
                max_age: Some(3600),
                secure: true,
                http_only: true,
                same_site: Some(SameSite::Lax),
            },
        };
        let header = format_set_cookie_header("__oxp_test_", &cookie);
        assert!(header.starts_with("__oxp_test_uid=xyz"));
        assert!(header.contains("Path=/"));
        assert!(header.contains("Domain=.example.com"));
        assert!(header.contains("Max-Age=3600"));
        assert!(header.contains("Secure"));
        assert!(header.contains("HttpOnly"));
        assert!(header.contains("SameSite=Lax"));
    }

    #[test]
    fn test_same_site_as_str() {
        assert_eq!(SameSite::Strict.as_str(), "Strict");
        assert_eq!(SameSite::Lax.as_str(), "Lax");
        assert_eq!(SameSite::None.as_str(), "None");
    }
}
