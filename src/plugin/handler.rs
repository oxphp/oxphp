use std::net::SocketAddr;
use std::time::Duration;

use http::{HeaderName, HeaderValue, Method, StatusCode, Uri};

use super::cookies::{CookieOptions, PluginCookies, PluginSetCookie};
use crate::events::Priority;
use crate::types::ResponseBody;

// ─── Blocked headers ─────────────────────────────────────────

/// Response headers that plugins cannot set.
const BLOCKED_RESPONSE_HEADERS: &[&str] = &[
    "set-cookie",
    "content-length",
    "transfer-encoding",
    "server",
    "x-request-id",
];

// ─── Request view/actions ────────────────────────────────────

/// Immutable view of the incoming request.
pub struct PluginRequestView<'a> {
    pub method: &'a Method,
    pub uri: &'a Uri,
    pub remote_addr: SocketAddr,
    pub request_id: &'a str,
    headers: &'a http::HeaderMap,
    cookies: PluginCookies,
    metadata: &'a [(String, String)],
}

impl<'a> PluginRequestView<'a> {
    pub(crate) fn new(
        method: &'a Method,
        uri: &'a Uri,
        remote_addr: SocketAddr,
        request_id: &'a str,
        headers: &'a http::HeaderMap,
        cookies: PluginCookies,
        metadata: &'a [(String, String)],
    ) -> Self {
        Self {
            method,
            uri,
            remote_addr,
            request_id,
            headers,
            cookies,
            metadata,
        }
    }

    /// Read a request header. `Cookie` header is blocked — use `cookie()`.
    pub fn header(&self, name: &str) -> Option<&HeaderValue> {
        if name.eq_ignore_ascii_case("cookie") {
            None
        } else {
            self.headers.get(name)
        }
    }

    /// Read this plugin's cookie by key (prefix applied automatically).
    pub fn cookie(&self, key: &str) -> Option<&str> {
        self.cookies.get(key)
    }

    /// Look up a metadata value by key.
    pub fn metadata(&self, key: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Actions a plugin can take during request processing.
pub struct PluginRequestActions {
    pub(crate) metadata: Vec<(String, String)>,
    pub(crate) early_response: Option<http::Response<ResponseBody>>,
    pub(crate) request_id_override: Option<String>,
}

impl PluginRequestActions {
    pub(crate) fn new() -> Self {
        Self {
            metadata: Vec::new(),
            early_response: None,
            request_id_override: None,
        }
    }

    /// Add metadata (visible to other plugins and core via event metadata).
    pub fn set_metadata(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.metadata.push((key.into(), value.into()));
    }

    /// Short-circuit the pipeline with an early response.
    pub fn set_early_response(&mut self, response: http::Response<ResponseBody>) {
        self.early_response = Some(response);
    }

    /// Override the request ID (e.g. with a trace-derived identifier).
    pub fn set_request_id(&mut self, id: String) {
        self.request_id_override = Some(id);
    }
}

// ─── Response view/actions ───────────────────────────────────

/// What a plugin sees after the response is built (read-only).
pub struct PluginResponseView<'a> {
    pub status: StatusCode,
    pub request_id: &'a str,
    headers: &'a http::HeaderMap,
    metadata: &'a [(String, String)],
}

impl<'a> PluginResponseView<'a> {
    pub(crate) fn new(
        status: StatusCode,
        request_id: &'a str,
        headers: &'a http::HeaderMap,
        metadata: &'a [(String, String)],
    ) -> Self {
        Self {
            status,
            request_id,
            headers,
            metadata,
        }
    }

    pub fn header(&self, name: &str) -> Option<&HeaderValue> {
        self.headers.get(name)
    }

    /// Look up a metadata value by key.
    pub fn metadata(&self, key: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

/// Actions a plugin can take on responses.
pub struct PluginResponseActions<'a> {
    plugin_name: &'a str,
    pub(crate) add_headers: Vec<(HeaderName, HeaderValue)>,
    pub(crate) set_cookies: Vec<PluginSetCookie>,
}

impl<'a> PluginResponseActions<'a> {
    pub(crate) fn new(plugin_name: &'a str) -> Self {
        Self {
            plugin_name,
            add_headers: Vec::new(),
            set_cookies: Vec::new(),
        }
    }

    /// Add a response header. Dangerous headers are silently blocked.
    pub fn add_header(&mut self, name: &str, value: HeaderValue) {
        if BLOCKED_RESPONSE_HEADERS
            .iter()
            .any(|blocked| blocked.eq_ignore_ascii_case(name))
        {
            tracing::warn!(plugin = self.plugin_name, header = name, "Blocked header");
            return;
        }
        if let Ok(header_name) = name.parse::<HeaderName>() {
            self.add_headers.push((header_name, value));
        }
    }

    /// Set a cookie (automatically prefixed with `__oxp_{plugin_name}_`).
    pub fn set_cookie(&mut self, key: &str, value: &str, opts: CookieOptions) {
        self.set_cookies.push(PluginSetCookie {
            key: key.to_string(),
            value: value.to_string(),
            opts,
        });
    }
}

// ─── Complete view ───────────────────────────────────────────

/// Immutable completion info (for logging / metrics).
pub struct PluginCompleteView<'a> {
    pub request_id: &'a str,
    pub method: &'a str,
    pub path: &'a str,
    pub status: u16,
    pub duration: Duration,
    pub remote_addr: SocketAddr,
    pub request_body_size: u64,
    pub response_size: u64,
    metadata: &'a [(String, String)],
    /// PHP errors captured during script execution (empty for static files).
    pub php_errors: &'a [crate::types::PhpScriptError],
    /// Serialized APM child spans (JSON). None when APM is disabled or no spans.
    pub apm_spans_json: Option<&'a str>,
    /// Time spent waiting in the worker queue (microseconds).
    pub queue_wait_us: Option<u64>,
    /// PHP script execution time (microseconds).
    pub php_exec_us: Option<u64>,
}

impl<'a> PluginCompleteView<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        request_id: &'a str,
        method: &'a str,
        path: &'a str,
        status: u16,
        duration: Duration,
        remote_addr: SocketAddr,
        request_body_size: u64,
        response_size: u64,
        metadata: &'a [(String, String)],
        php_errors: &'a [crate::types::PhpScriptError],
        apm_spans_json: Option<&'a str>,
        queue_wait_us: Option<u64>,
        php_exec_us: Option<u64>,
    ) -> Self {
        Self {
            request_id,
            method,
            path,
            status,
            duration,
            remote_addr,
            request_body_size,
            response_size,
            metadata,
            php_errors,
            apm_spans_json,
            queue_wait_us,
            php_exec_us,
        }
    }

    /// Look up a metadata value by key.
    pub fn metadata(&self, key: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }
}

// ─── Handler traits ──────────────────────────────────────────

/// Plugin handler for RequestReceived phase.
pub trait PluginRequestHandler: Send + Sync {
    fn handle(&self, view: &PluginRequestView, actions: &mut PluginRequestActions);
    fn priority(&self) -> Priority {
        0
    }
}

/// Plugin handler for ResponseBuilding phase.
pub trait PluginResponseHandler: Send + Sync {
    fn handle(&self, view: &PluginResponseView, actions: &mut PluginResponseActions<'_>);
    fn priority(&self) -> Priority {
        0
    }
}

/// Plugin handler for RequestComplete phase (read-only, for logging/metrics).
pub trait PluginCompleteHandler: Send + Sync {
    fn handle(&self, view: &PluginCompleteView);
    fn priority(&self) -> Priority {
        0
    }
}

// ─── Metrics collector ───────────────────────────────────────

/// Plugin metrics collector — appends Prometheus-formatted lines to output.
pub trait PluginMetricsCollector: Send + Sync {
    fn collect(&self, output: &mut String);
}

/// Closure adapter for PluginMetricsCollector.
impl<F: Fn(&mut String) + Send + Sync> PluginMetricsCollector for F {
    fn collect(&self, output: &mut String) {
        (self)(output)
    }
}

// ─── Internal handler ────────────────────────────────────────

/// Immutable view of an internal server request.
pub struct PluginInternalRequest<'a> {
    pub method: &'a Method,
    pub path: &'a str,
    pub headers: &'a http::HeaderMap,
    pub query: Option<&'a str>,
}

impl<'a> PluginInternalRequest<'a> {
    pub fn header(&self, name: &str) -> Option<&HeaderValue> {
        self.headers.get(name)
    }
}

/// Plugin handler for internal server routes.
pub trait PluginInternalHandler: Send + Sync {
    fn handle(&self, req: &PluginInternalRequest) -> http::Response<ResponseBody>;
}

/// Closure adapter for PluginInternalHandler.
impl<F> PluginInternalHandler for F
where
    F: Fn(&PluginInternalRequest) -> http::Response<ResponseBody> + Send + Sync,
{
    fn handle(&self, req: &PluginInternalRequest) -> http::Response<ResponseBody> {
        (self)(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_request_view_blocks_cookie_header() {
        let mut headers = http::HeaderMap::new();
        headers.insert("cookie", "session=abc".parse().unwrap());
        headers.insert("authorization", "Bearer tok".parse().unwrap());

        let cookies = PluginCookies { cookies: vec![] };
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let uri: Uri = "/test".parse().unwrap();
        let view =
            PluginRequestView::new(&Method::GET, &uri, addr, "req123", &headers, cookies, &[]);

        assert!(view.header("cookie").is_none());
        assert!(view.header("Cookie").is_none());
        assert!(view.header("COOKIE").is_none());
        assert!(view.header("authorization").is_some());
    }

    #[test]
    fn test_request_view_cookie_access() {
        let headers = http::HeaderMap::new();
        let cookies = PluginCookies {
            cookies: vec![("token".into(), "abc".into())],
        };
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let uri: Uri = "/test".parse().unwrap();
        let view =
            PluginRequestView::new(&Method::GET, &uri, addr, "req123", &headers, cookies, &[]);

        assert_eq!(view.cookie("token"), Some("abc"));
        assert_eq!(view.cookie("other"), None);
    }

    #[test]
    fn test_request_actions_metadata() {
        let mut actions = PluginRequestActions::new();
        actions.set_metadata("user_id", "42");
        assert_eq!(actions.metadata.len(), 1);
        assert_eq!(
            actions.metadata[0],
            ("user_id".to_string(), "42".to_string())
        );
    }

    #[test]
    fn test_response_view_header() {
        let mut headers = http::HeaderMap::new();
        headers.insert("x-custom", "val".parse().unwrap());
        let view = PluginResponseView::new(StatusCode::OK, "req123", &headers, &[]);
        assert_eq!(view.header("x-custom").unwrap(), "val");
        assert!(view.header("missing").is_none());
    }

    #[test]
    fn test_blocked_response_header() {
        let mut actions = PluginResponseActions::new("test");
        actions.add_header("set-cookie", "session=stolen".parse().unwrap());
        assert!(actions.add_headers.is_empty()); // blocked

        actions.add_header("Set-Cookie", "session=stolen".parse().unwrap());
        assert!(actions.add_headers.is_empty()); // blocked (case insensitive)

        actions.add_header("content-length", "100".parse().unwrap());
        assert!(actions.add_headers.is_empty()); // blocked

        actions.add_header("transfer-encoding", "chunked".parse().unwrap());
        assert!(actions.add_headers.is_empty()); // blocked

        actions.add_header("server", "evil".parse().unwrap());
        assert!(actions.add_headers.is_empty()); // blocked

        actions.add_header("x-request-id", "fake".parse().unwrap());
        assert!(actions.add_headers.is_empty()); // blocked
    }

    #[test]
    fn test_allowed_response_header() {
        let mut actions = PluginResponseActions::new("test");
        actions.add_header("x-custom", "value".parse().unwrap());
        assert_eq!(actions.add_headers.len(), 1);

        actions.add_header("access-control-allow-origin", "*".parse().unwrap());
        assert_eq!(actions.add_headers.len(), 2);
    }

    #[test]
    fn test_response_actions_set_cookie() {
        let mut actions = PluginResponseActions::new("test");
        actions.set_cookie("token", "abc", CookieOptions::default());
        assert_eq!(actions.set_cookies.len(), 1);
        assert_eq!(actions.set_cookies[0].key, "token");
        assert_eq!(actions.set_cookies[0].value, "abc");
    }

    #[test]
    fn test_internal_request_header() {
        let mut headers = http::HeaderMap::new();
        headers.insert("accept", "application/json".parse().unwrap());
        let req = PluginInternalRequest {
            method: &Method::GET,
            path: "/__test/status",
            headers: &headers,
            query: None,
        };
        assert!(req.header("accept").is_some());
    }

    #[test]
    fn test_request_view_metadata_accessor() {
        let headers = http::HeaderMap::new();
        let cookies = PluginCookies { cookies: vec![] };
        let addr: SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let uri: Uri = "/test".parse().unwrap();
        let metadata = vec![
            ("trace_id".to_string(), "abc123".to_string()),
            ("span_id".to_string(), "def456".to_string()),
        ];
        let view = PluginRequestView::new(
            &Method::GET,
            &uri,
            addr,
            "req123",
            &headers,
            cookies,
            &metadata,
        );
        assert_eq!(view.metadata("trace_id"), Some("abc123"));
        assert_eq!(view.metadata("span_id"), Some("def456"));
        assert_eq!(view.metadata("missing"), None);
    }

    #[test]
    fn test_request_id_override() {
        let mut actions = PluginRequestActions::new();
        assert!(actions.request_id_override.is_none());
        actions.set_request_id("trace-derived-id".to_string());
        assert_eq!(
            actions.request_id_override,
            Some("trace-derived-id".to_string())
        );
    }
}
