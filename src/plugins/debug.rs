use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::events::Priority;
use crate::plugin::cookies::{CookieOptions, SameSite};
use crate::plugin::handler::{
    PluginCompleteHandler, PluginCompleteView, PluginInternalRequest, PluginRequestActions,
    PluginRequestHandler, PluginRequestView, PluginResponseActions, PluginResponseHandler,
    PluginResponseView,
};
use crate::plugin::{
    PhpArray, PhpValue, Plugin, PluginContext, PluginDeps, PluginError, PluginHealth,
};
use crate::{php_array, php_call, php_function, php_object};

// ─── Plugin trait: 3 required + 4 optional ───────────────────

pub struct DebugPlugin {
    counter: Arc<AtomicU64>,
    internal_auth_b64: Option<String>,
}

impl Default for DebugPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl DebugPlugin {
    pub fn new() -> Self {
        Self {
            counter: Arc::new(AtomicU64::new(0)),
            internal_auth_b64: None,
        }
    }
}

impl Plugin for DebugPlugin {
    fn name(&self) -> &'static str {
        "debug"
    }

    fn version(&self) -> &'static str {
        "0.1.0"
    }

    fn init(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        let verbose = ctx.config("VERBOSE").map(|v| v == "true").unwrap_or(false);

        self.internal_auth_b64 = ctx.config("INTERNAL_AUTH").map(|creds| {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.encode(creds)
        });
        let auth_enabled = self.internal_auth_b64.is_some();

        ctx.register_service("debug_counter", Box::new(Arc::clone(&self.counter)));

        tracing::info!(plugin = ctx.plugin_name(), verbose, "debug plugin init");

        ctx.expose_config("verbose", verbose);
        ctx.expose_config("counter_service", "debug_counter");
        ctx.expose_config("internal_auth", auth_enabled);

        let counter = Arc::clone(&self.counter);
        ctx.register_metrics(move |output: &mut String| {
            use std::fmt::Write;
            let count = counter.load(Ordering::Relaxed);
            writeln!(
                output,
                "# HELP oxphp_plugin_debug_requests_total Requests seen by debug plugin"
            )
            .ok();
            writeln!(output, "# TYPE oxphp_plugin_debug_requests_total counter").ok();
            writeln!(output, "oxphp_plugin_debug_requests_total {count}").ok();
        });

        let counter = Arc::clone(&self.counter);
        let expected_auth = self.internal_auth_b64.clone();
        let version = self.version();
        // verbose is snapshot at init — runtime changes to VERBOSE are not reflected
        ctx.internal_route("/__debug/info", move |req: &PluginInternalRequest| {
            if let Some(ref expected) = expected_auth {
                let authorized = req
                    .header("authorization")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Basic "))
                    .map(|provided| provided == expected.as_str())
                    .unwrap_or(false);

                if !authorized {
                    return http::Response::builder()
                        .status(401)
                        .header("www-authenticate", "Basic realm=\"oxphp-debug\"")
                        .header("content-type", "text/plain")
                        .body(crate::types::full_body(bytes::Bytes::from_static(
                            b"401 Unauthorized\n",
                        )))
                        .expect("valid 401 response");
                }
            }

            let body = serde_json::json!({
                "plugin": "debug",
                "version": version,
                "requests_total": counter.load(Ordering::Relaxed),
                "verbose": verbose,
                "auth_enabled": expected_auth.is_some(),
            });
            http::Response::builder()
                .status(200)
                .header("content-type", "application/json")
                .body(crate::types::full_body(bytes::Bytes::from(
                    body.to_string(),
                )))
                .expect("valid JSON response")
        });

        // ─── PHP Function Registration ─────────────────────────

        // 1. Simple string transform
        php_function!(ctx, "echo_upper", fn(input: String) -> String {
            Ok(input.to_uppercase())
        });

        // 2. Call existing PHP functions
        php_function!(ctx, "json_roundtrip", fn(value: PhpValue) -> PhpValue {
            let encoded = php_call!(ctx, "json_encode", value.clone())?;
            let decoded = php_call!(ctx, "json_decode", encoded.clone(), true)?;
            Ok(php_array!({ "encoded" => encoded, "decoded" => decoded }))
        });

        // 3. Array inspection
        php_function!(ctx, "array_info", fn(arr: PhpArray) -> PhpValue {
            let keys = PhpArray::from_vec(arr.keys().map(|k| k.to_php_value()).collect());
            let types = PhpArray::from_vec(
                arr.values().map(|v| PhpValue::String(v.type_name().to_string())).collect(),
            );
            Ok(php_array!({
                "count"   => arr.len() as i64,
                "keys"    => keys,
                "is_list" => arr.is_list(),
                "types"   => types,
            }))
        });

        // 4. Object creation
        php_function!(ctx, "make_dto",
            fn(; name?: String = String::from("default")) -> PhpValue {
            let timestamp = php_call!(ctx, "microtime", true).unwrap_or(PhpValue::Float(0.0));
            Ok(php_object!({
                plugin:    "debug",
                name:      name,
                timestamp: timestamp,
                tags:      php_array!(["test", "debug"]),
            }))
        });

        // 5. Environment check
        php_function!(ctx, "env_check", fn() -> PhpValue {
            Ok(php_array!({
                "php_version" => php_call!(ctx, "phpversion")?,
                "extensions"  => php_array!({
                    "json"      => php_call!(ctx, "extension_loaded", "json")?,
                    "mbstring"  => php_call!(ctx, "extension_loaded", "mbstring")?,
                    "opcache"   => php_call!(ctx, "extension_loaded", "Zend OPcache")?,
                    "curl"      => php_call!(ctx, "extension_loaded", "curl")?,
                    "pdo"       => php_call!(ctx, "extension_loaded", "pdo")?,
                }),
                "functions" => php_array!({
                    "json_encode"   => php_call!(ctx, "function_exists", "json_encode")?,
                    "array_map"     => php_call!(ctx, "function_exists", "array_map")?,
                    "mb_strtoupper" => php_call!(ctx, "function_exists", "mb_strtoupper")?,
                }),
                "ini" => php_array!({
                    "memory_limit"       => php_call!(ctx, "ini_get", "memory_limit")?,
                    "max_execution_time" => php_call!(ctx, "ini_get", "max_execution_time")?,
                    "display_errors"     => php_call!(ctx, "ini_get", "display_errors")?,
                }),
            }))
        });

        // ─── HTTP Handlers ─────────────────────────────────────

        ctx.on_request(DebugRequestHandler {
            counter: Arc::clone(&self.counter),
            verbose,
        });
        ctx.on_response(DebugResponseHandler { verbose });
        ctx.on_complete(DebugCompleteHandler);

        Ok(())
    }

    fn on_ready(&self) {
        let total = self.counter.load(Ordering::Relaxed);
        tracing::info!(total, "debug plugin ready");
    }

    fn shutdown(&self) {
        let total = self.counter.load(Ordering::Relaxed);
        tracing::info!(total_requests = total, "debug plugin shutdown");
    }

    fn dependencies(&self) -> PluginDeps {
        PluginDeps {
            optional: vec!["otel"],
            ..Default::default()
        }
    }

    fn health(&self) -> PluginHealth {
        // Thresholds are arbitrary — purely to exercise the health API
        match self.counter.load(Ordering::Relaxed) {
            0..=999_999 => PluginHealth::Ok,
            1_000_000..=9_999_999 => PluginHealth::Degraded,
            _ => PluginHealth::Failed,
        }
    }
}

// ─── PluginRequestHandler ────────────────────────────────────

struct DebugRequestHandler {
    counter: Arc<AtomicU64>,
    verbose: bool,
}

impl PluginRequestHandler for DebugRequestHandler {
    fn handle(&self, view: &PluginRequestView, actions: &mut PluginRequestActions) {
        self.counter.fetch_add(1, Ordering::Relaxed);

        actions.set_metadata("debug_seen", "true");

        if view.method == http::Method::GET && view.uri.path() == "/__debug/teapot" {
            actions.set_early_response(
                http::Response::builder()
                    .status(418)
                    .header("content-type", "text/plain")
                    .body(crate::types::full_body(bytes::Bytes::from_static(
                        b"I'm a teapot\n",
                    )))
                    .expect("valid 418 response"),
            );
        }

        if self.verbose {
            tracing::debug!(
                request_id = view.request_id,
                method = %view.method,
                path = view.uri.path(),
                remote_addr = %view.remote_addr,
                "debug: request received"
            );
        }
    }

    fn priority(&self) -> Priority {
        50
    }
}

// ─── PluginResponseHandler ───────────────────────────────────

struct DebugResponseHandler {
    verbose: bool,
}

impl PluginResponseHandler for DebugResponseHandler {
    fn handle(&self, view: &PluginResponseView, actions: &mut PluginResponseActions<'_>) {
        actions.add_header("X-Debug-Plugin", http::HeaderValue::from_static("active"));

        actions.set_cookie(
            "visited",
            "true",
            CookieOptions {
                max_age: Some(3600),
                path: Some("/".to_string()),
                http_only: true,
                secure: false,
                same_site: Some(SameSite::Lax),
                ..Default::default()
            },
        );

        if self.verbose {
            tracing::debug!(status = view.status.as_u16(), "debug: response building");
        }
    }

    fn priority(&self) -> Priority {
        50
    }
}

// ─── PluginCompleteHandler ───────────────────────────────────

struct DebugCompleteHandler;

impl PluginCompleteHandler for DebugCompleteHandler {
    fn handle(&self, view: &PluginCompleteView) {
        tracing::debug!(
            request_id = view.request_id,
            method = view.method,
            path = view.path,
            status = view.status,
            duration_us = view.duration.as_micros() as u64,
            remote_addr = %view.remote_addr,
            "debug: request complete"
        );
    }

    fn priority(&self) -> Priority {
        90
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventDispatcher;
    use crate::plugin::cookies::PluginCookies;
    use crate::plugin::handler::{PluginInternalHandler, PluginMetricsCollector};
    use crate::plugin::php::{PhpCallContext, PhpError, PhpType, PluginPhpFunctionDef};
    use std::collections::HashMap;
    use std::net::SocketAddr;

    // ── Helper: build a PluginRequestView ──

    fn build_request_view(
        method: &str,
        path: &str,
    ) -> (http::Method, http::Uri, SocketAddr, String, http::HeaderMap) {
        let method: http::Method = method.parse().unwrap();
        let uri: http::Uri = path.parse().unwrap();
        let addr: SocketAddr = "127.0.0.1:12345".parse().unwrap();
        let rid = "abc123".to_string();
        let headers = http::HeaderMap::new();
        (method, uri, addr, rid, headers)
    }

    fn make_view<'a>(
        method: &'a http::Method,
        uri: &'a http::Uri,
        addr: SocketAddr,
        rid: &'a str,
        headers: &'a http::HeaderMap,
    ) -> PluginRequestView<'a> {
        PluginRequestView::new(
            method,
            uri,
            addr,
            rid,
            headers,
            PluginCookies { cookies: vec![] },
        )
    }

    // Mutex to serialize tests that manipulate env vars (tests run in parallel)
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    // ── Helper: init plugin and return extracted state ──

    struct InitState {
        services: HashMap<String, Box<dyn std::any::Any + Send + Sync>>,
        config_values: HashMap<String, serde_json::Value>,
        metrics_collectors: Vec<Box<dyn PluginMetricsCollector>>,
        internal_routes: HashMap<String, Box<dyn PluginInternalHandler>>,
        php_functions: Vec<PluginPhpFunctionDef>,
    }

    fn init_plugin(plugin: &mut DebugPlugin) -> InitState {
        let mut dispatcher = EventDispatcher::new();
        let mut services: HashMap<String, Box<dyn std::any::Any + Send + Sync>> = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics_collectors: Vec<Box<dyn PluginMetricsCollector>> = Vec::new();
        let mut internal_routes: HashMap<String, Box<dyn PluginInternalHandler>> = HashMap::new();
        let mut php_functions = Vec::new();

        let mut ctx = PluginContext::new(
            "debug".into(),
            "__oxp_debug_".into(),
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics_collectors,
            &mut internal_routes,
            &mut php_functions,
        );
        plugin.init(&mut ctx).unwrap();
        drop(ctx);

        InitState {
            services,
            config_values,
            metrics_collectors,
            internal_routes,
            php_functions,
        }
    }

    fn init_debug_plugin() -> (DebugPlugin, InitState) {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut plugin = DebugPlugin::new();
        let state = init_plugin(&mut plugin);
        (plugin, state)
    }

    // ── Plugin lifecycle ──

    #[test]
    fn test_debug_plugin_lifecycle() {
        let (plugin, state) = init_debug_plugin();

        assert_eq!(plugin.name(), "debug");
        assert_eq!(plugin.version(), "0.1.0");
        assert!(state.services.contains_key("debug_counter"));
        assert_eq!(plugin.health(), PluginHealth::Ok);

        plugin.on_ready();
        plugin.shutdown();
    }

    // ── Health degradation ──

    #[test]
    fn test_debug_health_degrades() {
        let plugin = DebugPlugin::new();
        assert_eq!(plugin.health(), PluginHealth::Ok);

        plugin.counter.store(1_500_000, Ordering::Relaxed);
        assert_eq!(plugin.health(), PluginHealth::Degraded);

        plugin.counter.store(10_000_000, Ordering::Relaxed);
        assert_eq!(plugin.health(), PluginHealth::Failed);
    }

    // ── Dependencies ──

    #[test]
    fn test_debug_dependencies() {
        let plugin = DebugPlugin::new();
        let deps = plugin.dependencies();
        assert!(deps.required.is_empty());
        assert_eq!(deps.optional, vec!["otel"]);
        assert!(deps.services.is_empty());
    }

    // ── Teapot early response ──

    #[test]
    fn test_debug_early_response_teapot() {
        let handler = DebugRequestHandler {
            counter: Arc::new(AtomicU64::new(0)),
            verbose: false,
        };

        let (method, uri, addr, rid, headers) = build_request_view("GET", "/__debug/teapot");
        let view = make_view(&method, &uri, addr, &rid, &headers);
        let mut actions = PluginRequestActions::new();

        handler.handle(&view, &mut actions);

        assert!(actions.early_response.is_some());
        assert_eq!(actions.early_response.unwrap().status(), 418);
    }

    // ── Normal request (no early response, 4 metadata keys) ──

    #[test]
    fn test_debug_normal_request() {
        let handler = DebugRequestHandler {
            counter: Arc::new(AtomicU64::new(0)),
            verbose: false,
        };

        let (method, uri, addr, rid, headers) = build_request_view("GET", "/index.php");
        let view = make_view(&method, &uri, addr, &rid, &headers);
        let mut actions = PluginRequestActions::new();

        handler.handle(&view, &mut actions);

        assert!(actions.early_response.is_none());
        assert_eq!(actions.metadata.len(), 1);

        let meta: HashMap<_, _> = actions.metadata.into_iter().collect();
        assert_eq!(meta["debug_seen"], "true");
    }

    // ── Counter increments ──

    #[test]
    fn test_debug_counter_increments() {
        let counter = Arc::new(AtomicU64::new(0));
        let handler = DebugRequestHandler {
            counter: Arc::clone(&counter),
            verbose: false,
        };

        let (method, uri, addr, rid, headers) = build_request_view("GET", "/");
        let view = make_view(&method, &uri, addr, &rid, &headers);

        for _ in 0..3 {
            let mut actions = PluginRequestActions::new();
            handler.handle(&view, &mut actions);
        }
        assert_eq!(counter.load(Ordering::Relaxed), 3);
    }

    // ── Response handler ──

    #[test]
    fn test_debug_response_handler() {
        let handler = DebugResponseHandler { verbose: false };
        let resp_headers = http::HeaderMap::new();
        let view = PluginResponseView::new(http::StatusCode::OK, "req123", &resp_headers);
        let mut actions = PluginResponseActions::new("debug");

        handler.handle(&view, &mut actions);

        assert_eq!(actions.add_headers.len(), 1);
        assert_eq!(actions.set_cookies.len(), 1);
        assert_eq!(actions.add_headers[0].0.as_str(), "x-debug-plugin");
    }

    // ── Cookie set via response ──

    #[test]
    fn test_debug_cookie_set_via_response() {
        let handler = DebugResponseHandler { verbose: false };
        let resp_headers = http::HeaderMap::new();
        let view = PluginResponseView::new(http::StatusCode::OK, "req1", &resp_headers);
        let mut actions = PluginResponseActions::new("debug");
        handler.handle(&view, &mut actions);

        assert_eq!(actions.set_cookies.len(), 1);
        assert_eq!(actions.set_cookies[0].key, "visited");
        assert_eq!(actions.set_cookies[0].value, "true");
    }

    // ── Config exposed ──

    #[test]
    fn test_debug_config_exposed() {
        let (_plugin, state) = init_debug_plugin();

        assert_eq!(state.config_values["verbose"], serde_json::json!(false));
        assert_eq!(
            state.config_values["counter_service"],
            serde_json::json!("debug_counter")
        );
        assert_eq!(
            state.config_values["internal_auth"],
            serde_json::json!(false)
        );
    }

    // ── Metrics collected ──

    #[test]
    fn test_debug_metrics_collected() {
        let (_plugin, state) = init_debug_plugin();

        assert_eq!(state.metrics_collectors.len(), 1);
        let mut output = String::new();
        state.metrics_collectors[0].collect(&mut output);

        assert!(output.contains("oxphp_plugin_debug_requests_total"));
        assert!(output.contains("# TYPE oxphp_plugin_debug_requests_total counter"));
    }

    // ── Internal route (no auth) ──

    #[test]
    fn test_debug_internal_route_no_auth() {
        let (_plugin, state) = init_debug_plugin();

        let handler = state.internal_routes.get("/__debug/info").unwrap();
        let headers = http::HeaderMap::new();
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__debug/info",
            headers: &headers,
            query: None,
        };
        let response = handler.handle(&req);
        assert_eq!(response.status(), 200);
    }

    // ── Internal route (with auth) ──

    fn init_debug_plugin_with_auth(creds: &str) -> InitState {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("DEBUG_INTERNAL_AUTH", creds);
        let mut plugin = DebugPlugin::new();
        let state = init_plugin(&mut plugin);
        std::env::remove_var("DEBUG_INTERNAL_AUTH");
        state
    }

    #[test]
    fn test_debug_internal_route_auth_required() {
        let state = init_debug_plugin_with_auth("admin:secret");
        let handler = state.internal_routes.get("/__debug/info").unwrap();

        // No auth → 401
        let headers = http::HeaderMap::new();
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__debug/info",
            headers: &headers,
            query: None,
        };
        let response = handler.handle(&req);
        assert_eq!(response.status(), 401);
        assert!(response.headers().get("www-authenticate").is_some());

        // Wrong credentials → 401
        let mut headers = http::HeaderMap::new();
        headers.insert("authorization", "Basic d3Jvbmc6Y3JlZHM=".parse().unwrap());
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__debug/info",
            headers: &headers,
            query: None,
        };
        let response = handler.handle(&req);
        assert_eq!(response.status(), 401);

        // Correct credentials → 200
        use base64::Engine;
        let encoded = base64::engine::general_purpose::STANDARD.encode("admin:secret");
        let mut headers = http::HeaderMap::new();
        headers.insert("authorization", format!("Basic {encoded}").parse().unwrap());
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__debug/info",
            headers: &headers,
            query: None,
        };
        let response = handler.handle(&req);
        assert_eq!(response.status(), 200);
    }

    // ── PHP functions registered ──

    #[test]
    fn test_debug_php_functions_registered() {
        let (_plugin, state) = init_debug_plugin();

        assert_eq!(state.php_functions.len(), 5);
        assert_eq!(state.php_functions[0].name, "oxphp_debug_echo_upper");
        assert_eq!(state.php_functions[1].name, "oxphp_debug_json_roundtrip");
        assert_eq!(state.php_functions[2].name, "oxphp_debug_array_info");
        assert_eq!(state.php_functions[3].name, "oxphp_debug_make_dto");
        assert_eq!(state.php_functions[4].name, "oxphp_debug_env_check");

        // echo_upper: 1 required param
        assert_eq!(state.php_functions[0].params.len(), 1);
        assert_eq!(state.php_functions[0].params[0].name, "input");
        assert!(state.php_functions[0].params[0].required);
        assert_eq!(state.php_functions[0].return_type, PhpType::String);

        // make_dto: 1 optional param
        assert!(!state.php_functions[3].params[0].required);

        // env_check: no params
        assert_eq!(state.php_functions[4].params.len(), 0);
    }

    // ── echo_upper handler ──

    #[test]
    fn test_debug_echo_upper_handler() {
        let (_plugin, state) = init_debug_plugin();
        let ctx = PhpCallContext::new();
        let handler = &state.php_functions[0].handler;

        let result = handler.handle(&ctx, &[PhpValue::String("hello world".into())]);
        assert_eq!(result.unwrap(), PhpValue::String("HELLO WORLD".into()));

        // Wrong type → TypeError
        let result = handler.handle(&ctx, &[PhpValue::Int(42)]);
        assert!(matches!(result, Err(PhpError::TypeError { .. })));
    }

    // ── array_info handler ──

    #[test]
    fn test_debug_array_info_handler() {
        let (_plugin, state) = init_debug_plugin();
        let ctx = PhpCallContext::new();
        let handler = &state.php_functions[2].handler;

        let input = PhpValue::Array(PhpArray::from_vec(vec![
            PhpValue::String("a".into()),
            PhpValue::String("b".into()),
        ]));
        let result = handler.handle(&ctx, &[input]).unwrap();

        let arr = result.as_array().unwrap();
        assert_eq!(arr.get("count"), Some(&PhpValue::Int(2)));
        assert_eq!(arr.get("is_list"), Some(&PhpValue::Bool(true)));
    }

    // ── make_dto handler ──

    #[test]
    fn test_debug_make_dto_handler_default() {
        let (_plugin, state) = init_debug_plugin();
        let ctx = PhpCallContext::new();
        let handler = &state.php_functions[3].handler;

        // No args → uses default "default"
        let result = handler.handle(&ctx, &[]).unwrap();
        let obj = result.as_object().unwrap();
        assert_eq!(obj.class_name, "stdClass");
        assert_eq!(obj.get("plugin"), Some(&PhpValue::String("debug".into())));
        assert_eq!(obj.get("name"), Some(&PhpValue::String("default".into())));
        assert_eq!(obj.get("tags").unwrap().as_array().unwrap().len(), 2);
    }

    // ── Complete handler (smoke test) ──

    #[test]
    fn test_debug_complete_handler() {
        let handler = DebugCompleteHandler;
        let view = PluginCompleteView {
            request_id: "req123",
            method: "GET",
            path: "/test",
            status: 200,
            duration: std::time::Duration::from_millis(42),
            remote_addr: "127.0.0.1:12345".parse().unwrap(),
        };
        handler.handle(&view); // should not panic
    }

    // ── Handler priorities ──

    #[test]
    fn test_debug_handler_priorities() {
        let req_handler = DebugRequestHandler {
            counter: Arc::new(AtomicU64::new(0)),
            verbose: false,
        };
        assert_eq!(req_handler.priority(), 50);

        let resp_handler = DebugResponseHandler { verbose: false };
        assert_eq!(resp_handler.priority(), 50);

        let complete_handler = DebugCompleteHandler;
        assert_eq!(complete_handler.priority(), 90);
    }
}
