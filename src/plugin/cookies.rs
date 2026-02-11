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
