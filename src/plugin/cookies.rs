use http::header::COOKIE;
use http::HeaderMap;

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

/// Find a cookie by its **full** name in the `Cookie` header, outside any
/// plugin's `__oxp_{name}_` namespace.
///
/// [`extract_plugin_cookies`] is the namespaced read, and it is what a plugin
/// gets through `PluginRequestView::cookie`. This one exists for a cookie
/// whose whole name is part of a documented interface an operator types by
/// hand, which a per-plugin prefix would put out of reach.
///
/// The `Cookie` field lines are scanned in the order they arrived and the
/// first pair whose name matches wins; a line carrying a byte outside visible
/// ASCII is skipped rather than ending the search. Over HTTP/1.1 a conforming
/// user agent sends only one such header (RFC 6265 section 5.4), but an
/// HTTP/2 client may legitimately split its cookies across several field lines
/// for better HPACK compression, and RFC 9113 section 8.2.3 puts the job of
/// joining them back together with `"; "` on the server. Nothing below us does
/// it — neither `h2` nor `hyper` rejoins them — so reading only the first field
/// line would lose cookies sent by a conforming client. Scanning the lines in
/// order stands in for scanning that concatenation, without building it.
///
/// `extract_plugin_cookies` and [`strip_plugin_cookies`] do still read only the
/// first field line, so this function sees cookies they do not.
///
/// The name is matched whole: a cookie called `__oxp_profiler_OXPROF` is not a
/// match for `OXPROF`. Nothing here strips the cookie from the request either,
/// so — unlike a `__oxp_*` one — the application still sees it: in `$_COOKIE`
/// when the client sent its cookies on one field line, and otherwise in
/// `$_SERVER['HTTP_COOKIE']`, because the SAPI builds `$_COOKIE` from the first
/// field line alone while `$_SERVER` is built from the last.
#[allow(dead_code)] // consumed by feature-gated plugins
pub(crate) fn find_raw_cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .find_map(|cookie_str| {
            cookie_str.split(';').find_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                if k.trim() == name {
                    Some(v.trim())
                } else {
                    None
                }
            })
        })
}

/// Strip all `__oxp_*` cookies from the Cookie header before PHP sees them.
pub fn strip_plugin_cookies(parts: &mut http::request::Parts) {
    let cookie_header = match parts.headers.get(COOKIE) {
        Some(h) => h,
        None => return,
    };

    let cookie_str = match cookie_header.to_str() {
        Ok(s) => s,
        Err(_) => return,
    };

    // Quick check: any __oxp_ cookies present?
    if !cookie_str.contains("__oxp_") {
        return;
    }

    let filtered: Vec<&str> = cookie_str
        .split(';')
        .filter(|pair| !pair.trim_start().starts_with("__oxp_"))
        .collect();

    if filtered.is_empty() {
        parts.headers.remove(COOKIE);
    } else {
        let new_value = filtered
            .iter()
            .map(|s| s.trim())
            .collect::<Vec<_>>()
            .join("; ");
        if let Ok(hv) = http::HeaderValue::from_str(&new_value) {
            parts.headers.insert(COOKIE, hv);
        }
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

    // ── find_raw_cookie tests ──

    #[test]
    fn test_find_raw_cookie_among_others() {
        let headers = make_headers("session=abc; OXPROF=tok; theme=dark");
        assert_eq!(find_raw_cookie(&headers, "OXPROF"), Some("tok"));
        assert_eq!(find_raw_cookie(&headers, "session"), Some("abc"));
        assert_eq!(find_raw_cookie(&headers, "absent"), None);
    }

    #[test]
    fn test_find_raw_cookie_matches_the_whole_name() {
        // Not a prefix match in either direction: the namespaced spelling is a
        // different cookie, and a name this one merely starts with is not it.
        let headers = make_headers("__oxp_profiler_OXPROF=tok; OXPROFX=other");
        assert_eq!(find_raw_cookie(&headers, "OXPROF"), None);
    }

    #[test]
    fn test_find_raw_cookie_ignores_surrounding_space() {
        // `; ` between pairs is what every user agent sends.
        let headers = make_headers("a=1;   OXPROF = tok  ; b=2");
        assert_eq!(find_raw_cookie(&headers, "OXPROF"), Some("tok"));
    }

    #[test]
    fn test_find_raw_cookie_no_cookie_header() {
        assert_eq!(find_raw_cookie(&HeaderMap::new(), "OXPROF"), None);
    }

    #[test]
    fn test_find_raw_cookie_takes_the_first_of_a_duplicate() {
        let headers = make_headers("OXPROF=first; OXPROF=second");
        assert_eq!(find_raw_cookie(&headers, "OXPROF"), Some("first"));
    }

    #[test]
    fn test_find_raw_cookie_skips_a_pair_with_no_value() {
        let headers = make_headers("flag; OXPROF=tok");
        assert_eq!(find_raw_cookie(&headers, "OXPROF"), Some("tok"));
    }

    #[test]
    fn test_find_raw_cookie_reads_a_split_cookie_header() {
        // An HTTP/2 client may split `Cookie` across several field lines, and
        // nothing below us rejoins them. The cookie is found wherever it lands.
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, "session=abc".parse().unwrap());
        headers.append(COOKIE, "OXPROF=tok".parse().unwrap());
        assert_eq!(find_raw_cookie(&headers, "OXPROF"), Some("tok"));
        assert_eq!(find_raw_cookie(&headers, "session"), Some("abc"));
        assert_eq!(find_raw_cookie(&headers, "absent"), None);
    }

    #[test]
    fn test_find_raw_cookie_skips_an_unreadable_field_line() {
        // A byte outside visible ASCII costs that line, not the whole lookup —
        // the cookie is still found on a later one.
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, http::HeaderValue::from_bytes(b"junk=\xff").unwrap());
        headers.append(COOKIE, "OXPROF=tok".parse().unwrap());
        assert_eq!(find_raw_cookie(&headers, "OXPROF"), Some("tok"));
    }

    #[test]
    fn test_find_raw_cookie_takes_the_first_across_split_headers() {
        // Same rule as within one header, applied to the concatenation the
        // field lines stand for: the earlier pair wins.
        let mut headers = HeaderMap::new();
        headers.append(COOKIE, "OXPROF=first".parse().unwrap());
        headers.append(COOKIE, "OXPROF=second".parse().unwrap());
        assert_eq!(find_raw_cookie(&headers, "OXPROF"), Some("first"));
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
