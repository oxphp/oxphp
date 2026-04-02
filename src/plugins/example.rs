use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crate::bridge::call::NativeCall;
use crate::bridge::types::ValType;
use crate::events::Priority;
use crate::plugin::cookies::{CookieOptions, SameSite};
use crate::plugin::handler::{
    PluginCompleteHandler, PluginCompleteView, PluginInternalRequest, PluginRequestActions,
    PluginRequestHandler, PluginRequestView, PluginResponseActions, PluginResponseHandler,
    PluginResponseView,
};
use crate::plugin::php::{PhpParam, PhpType};
use crate::plugin::{Plugin, PluginContext, PluginDeps, PluginError, PluginHealth};

// ─── Plugin trait: 3 required + 4 optional ───────────────────

pub struct ExamplePlugin {
    counter: Arc<AtomicU64>,
    internal_auth_b64: Option<String>,
}

impl Default for ExamplePlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ExamplePlugin {
    pub fn new() -> Self {
        Self {
            counter: Arc::new(AtomicU64::new(0)),
            internal_auth_b64: None,
        }
    }
}

impl Plugin for ExamplePlugin {
    fn name(&self) -> &'static str {
        "example"
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

        ctx.register_service("example_counter", Box::new(Arc::clone(&self.counter)));

        tracing::info!(plugin = ctx.plugin_name(), verbose, "example plugin init");

        ctx.expose_config("verbose", verbose);
        ctx.expose_config("counter_service", "example_counter");
        ctx.expose_config("internal_auth", auth_enabled);

        let counter = Arc::clone(&self.counter);
        ctx.register_metrics(move |output: &mut String| {
            use std::fmt::Write;
            let count = counter.load(Ordering::Relaxed);
            writeln!(
                output,
                "# HELP oxphp_plugin_example_requests_total Requests seen by example plugin"
            )
            .ok();
            writeln!(output, "# TYPE oxphp_plugin_example_requests_total counter").ok();
            writeln!(output, "oxphp_plugin_example_requests_total {count}").ok();
        });

        let counter = Arc::clone(&self.counter);
        let expected_auth = self.internal_auth_b64.clone();
        let version = self.version();
        // verbose is snapshot at init — runtime changes to VERBOSE are not reflected
        ctx.internal_route("/__example/info", move |req: &PluginInternalRequest| {
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
                        .header("www-authenticate", "Basic realm=\"oxphp-example\"")
                        .header("content-type", "text/plain")
                        .body(crate::types::full_body(bytes::Bytes::from_static(
                            b"401 Unauthorized\n",
                        )))
                        .expect("valid 401 response");
                }
            }

            let body = serde_json::json!({
                "plugin": "example",
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

        // ─── PHP Function Registration (Native Bridge) ──────────

        // 1. Simple string transform — exercises arg_str + ret_str
        ctx.register_function(
            "echo_upper",
            vec![PhpParam::required("input", PhpType::String)],
            PhpType::String,
            |call: &mut NativeCall| {
                let input = call.arg_str(0)?;
                let upper = input.to_uppercase();
                call.ret_str(&upper);
                Ok(())
            },
        );

        // 2. String info — exercises arg_str + call_php + ret_array
        ctx.register_function(
            "string_info",
            vec![PhpParam::required("input", PhpType::String)],
            PhpType::Array,
            |call: &mut NativeCall| {
                let input = call.arg_str(0)?;
                let len = input.len() as i64;
                let upper = input.to_uppercase();
                call.ret_array(3, |b| {
                    b.str("original", input);
                    b.str("upper", &upper);
                    b.long("length", len);
                });
                Ok(())
            },
        );

        // 3. Array inspection — exercises arg_array_foreach + ret_array
        ctx.register_function(
            "array_info",
            vec![PhpParam::required("arr", PhpType::Array)],
            PhpType::Array,
            |call: &mut NativeCall| {
                let count = call.arg_array_count(0)? as i64;

                // Collect type names via iteration
                let mut type_names: Vec<String> = Vec::new();
                let mut key_count = 0i64;
                call.arg_array_foreach(0, |_key, val| {
                    key_count += 1;
                    let tname = match val.val_type() {
                        ValType::Null => "null",
                        ValType::True | ValType::False => "bool",
                        ValType::Long => "int",
                        ValType::Double => "float",
                        ValType::String => "string",
                        ValType::Array => "array",
                        ValType::Object => "object",
                        ValType::Resource => "resource",
                    };
                    type_names.push(tname.to_string());
                })?;

                call.ret_array(3, |b| {
                    b.long("count", count);
                    b.long("key_count", key_count);
                    b.array("types", type_names.len() as u32, |tb| {
                        for t in &type_names {
                            tb.push_str(t);
                        }
                    });
                });
                Ok(())
            },
        );

        // 4. DTO builder — exercises optional arg + nested arrays
        ctx.register_function(
            "make_dto",
            vec![PhpParam::optional("name", PhpType::String, "default")],
            PhpType::Array,
            |call: &mut NativeCall| {
                let name = if call.argc() > 0 && !call.arg_is_null(0)? {
                    call.arg_str(0)?.to_string()
                } else {
                    "default".to_string()
                };
                call.ret_array(4, |b| {
                    b.str("plugin", "example");
                    b.str("name", &name);
                    b.array("tags", 2, |tb| {
                        tb.push_str("test");
                        tb.push_str("example");
                    });
                    b.bool("active", true);
                });
                Ok(())
            },
        );

        // 5. Environment check — exercises call_php + nested ret_array
        ctx.register_function(
            "env_check",
            vec![],
            PhpType::Array,
            |call: &mut NativeCall| {
                // Get PHP version string
                let php_version = call
                    .call_php("phpversion", 0, |_| {})
                    .ok()
                    .and_then(|r| r.val().as_str().map(String::from))
                    .unwrap_or_else(|| "unknown".into());

                call.ret_array(2, |b| {
                    b.str("php_version", &php_version);
                    b.str("bridge", "native");
                });
                Ok(())
            },
        );

        // ─── HTTP Handlers ─────────────────────────────────────

        ctx.on_request(ExampleRequestHandler {
            counter: Arc::clone(&self.counter),
            verbose,
        });
        ctx.on_response(ExampleResponseHandler { verbose });
        ctx.on_complete(ExampleCompleteHandler);

        Ok(())
    }

    fn on_ready(&self) {
        let total = self.counter.load(Ordering::Relaxed);
        tracing::info!(total, "example plugin ready");
    }

    fn shutdown(&self) {
        let total = self.counter.load(Ordering::Relaxed);
        tracing::info!(total_requests = total, "example plugin shutdown");
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

struct ExampleRequestHandler {
    counter: Arc<AtomicU64>,
    verbose: bool,
}

impl PluginRequestHandler for ExampleRequestHandler {
    fn handle(&self, view: &PluginRequestView, actions: &mut PluginRequestActions) {
        self.counter.fetch_add(1, Ordering::Relaxed);

        actions.set_metadata("example_seen", "true");

        if view.method == http::Method::GET && view.uri.path() == "/__example/teapot" {
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
                "example:request received"
            );
        }
    }

    fn priority(&self) -> Priority {
        50
    }
}

// ─── PluginResponseHandler ───────────────────────────────────

struct ExampleResponseHandler {
    verbose: bool,
}

impl PluginResponseHandler for ExampleResponseHandler {
    fn handle(&self, view: &PluginResponseView, actions: &mut PluginResponseActions<'_>) {
        actions.add_header("X-Example-Plugin", http::HeaderValue::from_static("active"));

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
            tracing::debug!(status = view.status.as_u16(), "example:response building");
        }
    }

    fn priority(&self) -> Priority {
        50
    }
}

// ─── PluginCompleteHandler ───────────────────────────────────

struct ExampleCompleteHandler;

impl PluginCompleteHandler for ExampleCompleteHandler {
    fn handle(&self, view: &PluginCompleteView) {
        tracing::debug!(
            request_id = view.request_id,
            method = view.method,
            path = view.path,
            status = view.status,
            duration_us = view.duration.as_micros() as u64,
            remote_addr = %view.remote_addr,
            "example:request complete"
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
    use crate::plugin::php::PluginNativeFunctionDef;
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
            &[],
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
        native_php_functions: Vec<PluginNativeFunctionDef>,
    }

    fn init_example(plugin: &mut ExamplePlugin) -> InitState {
        let mut dispatcher = EventDispatcher::new();
        let mut services: HashMap<String, Box<dyn std::any::Any + Send + Sync>> = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics_collectors: Vec<Box<dyn PluginMetricsCollector>> = Vec::new();
        let mut internal_routes: HashMap<String, Box<dyn PluginInternalHandler>> = HashMap::new();
        let mut native_php_functions = Vec::new();
        let mut decorators = Vec::new();

        let mut ctx = PluginContext::new(
            "example".into(),
            "__oxp_example_".into(),
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics_collectors,
            &mut internal_routes,
            &mut native_php_functions,
            &mut decorators,
        );
        plugin.init(&mut ctx).unwrap();
        drop(ctx);

        InitState {
            services,
            config_values,
            metrics_collectors,
            internal_routes,
            native_php_functions,
        }
    }

    fn init_example_plugin() -> (ExamplePlugin, InitState) {
        let _lock = ENV_MUTEX.lock().unwrap();
        let mut plugin = ExamplePlugin::new();
        let state = init_example(&mut plugin);
        (plugin, state)
    }

    // ── Plugin lifecycle ──

    #[test]
    fn test_example_plugin_lifecycle() {
        let (plugin, state) = init_example_plugin();

        assert_eq!(plugin.name(), "example");
        assert_eq!(plugin.version(), "0.1.0");
        assert!(state.services.contains_key("example_counter"));
        assert_eq!(plugin.health(), PluginHealth::Ok);

        plugin.on_ready();
        plugin.shutdown();
    }

    // ── Health degradation ──

    #[test]
    fn test_example_health_degrades() {
        let plugin = ExamplePlugin::new();
        assert_eq!(plugin.health(), PluginHealth::Ok);

        plugin.counter.store(1_500_000, Ordering::Relaxed);
        assert_eq!(plugin.health(), PluginHealth::Degraded);

        plugin.counter.store(10_000_000, Ordering::Relaxed);
        assert_eq!(plugin.health(), PluginHealth::Failed);
    }

    // ── Dependencies ──

    #[test]
    fn test_example_dependencies() {
        let plugin = ExamplePlugin::new();
        let deps = plugin.dependencies();
        assert!(deps.required.is_empty());
        assert_eq!(deps.optional, vec!["otel"]);
        assert!(deps.services.is_empty());
    }

    // ── Teapot early response ──

    #[test]
    fn test_example_early_response_teapot() {
        let handler = ExampleRequestHandler {
            counter: Arc::new(AtomicU64::new(0)),
            verbose: false,
        };

        let (method, uri, addr, rid, headers) = build_request_view("GET", "/__example/teapot");
        let view = make_view(&method, &uri, addr, &rid, &headers);
        let mut actions = PluginRequestActions::new();

        handler.handle(&view, &mut actions);

        assert!(actions.early_response.is_some());
        assert_eq!(actions.early_response.unwrap().status(), 418);
    }

    // ── Normal request (no early response) ──

    #[test]
    fn test_example_normal_request() {
        let handler = ExampleRequestHandler {
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
        assert_eq!(meta["example_seen"], "true");
    }

    // ── Counter increments ──

    #[test]
    fn test_example_counter_increments() {
        let counter = Arc::new(AtomicU64::new(0));
        let handler = ExampleRequestHandler {
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
    fn test_example_response_handler() {
        let handler = ExampleResponseHandler { verbose: false };
        let resp_headers = http::HeaderMap::new();
        let view = PluginResponseView::new(http::StatusCode::OK, "req123", &resp_headers, &[]);
        let mut actions = PluginResponseActions::new("example");

        handler.handle(&view, &mut actions);

        assert_eq!(actions.add_headers.len(), 1);
        assert_eq!(actions.set_cookies.len(), 1);
        assert_eq!(actions.add_headers[0].0.as_str(), "x-example-plugin");
    }

    // ── Cookie set via response ──

    #[test]
    fn test_example_cookie_set_via_response() {
        let handler = ExampleResponseHandler { verbose: false };
        let resp_headers = http::HeaderMap::new();
        let view = PluginResponseView::new(http::StatusCode::OK, "req1", &resp_headers, &[]);
        let mut actions = PluginResponseActions::new("example");
        handler.handle(&view, &mut actions);

        assert_eq!(actions.set_cookies.len(), 1);
        assert_eq!(actions.set_cookies[0].key, "visited");
        assert_eq!(actions.set_cookies[0].value, "true");
    }

    // ── Config exposed ──

    #[test]
    fn test_example_config_exposed() {
        let (_plugin, state) = init_example_plugin();

        assert_eq!(state.config_values["verbose"], serde_json::json!(false));
        assert_eq!(
            state.config_values["counter_service"],
            serde_json::json!("example_counter")
        );
        assert_eq!(
            state.config_values["internal_auth"],
            serde_json::json!(false)
        );
    }

    // ── Metrics collected ──

    #[test]
    fn test_example_metrics_collected() {
        let (_plugin, state) = init_example_plugin();

        assert_eq!(state.metrics_collectors.len(), 1);
        let mut output = String::new();
        state.metrics_collectors[0].collect(&mut output);

        assert!(output.contains("oxphp_plugin_example_requests_total"));
        assert!(output.contains("# TYPE oxphp_plugin_example_requests_total counter"));
    }

    // ── Internal route (no auth) ──

    #[test]
    fn test_example_internal_route_no_auth() {
        let (_plugin, state) = init_example_plugin();

        let handler = state.internal_routes.get("/__example/info").unwrap();
        let headers = http::HeaderMap::new();
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__example/info",
            headers: &headers,
            query: None,
        };
        let response = handler.handle(&req);
        assert_eq!(response.status(), 200);
    }

    // ── Internal route (with auth) ──

    fn init_example_plugin_with_auth(creds: &str) -> InitState {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("EXAMPLE_INTERNAL_AUTH", creds);
        let mut plugin = ExamplePlugin::new();
        let state = init_example(&mut plugin);
        std::env::remove_var("EXAMPLE_INTERNAL_AUTH");
        state
    }

    #[test]
    fn test_example_internal_route_auth_required() {
        let state = init_example_plugin_with_auth("admin:secret");
        let handler = state.internal_routes.get("/__example/info").unwrap();

        // No auth → 401
        let headers = http::HeaderMap::new();
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__example/info",
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
            path: "/__example/info",
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
            path: "/__example/info",
            headers: &headers,
            query: None,
        };
        let response = handler.handle(&req);
        assert_eq!(response.status(), 200);
    }

    // ── Native PHP functions registered ──

    #[test]
    fn test_example_native_php_functions_registered() {
        let (_plugin, state) = init_example_plugin();

        assert_eq!(state.native_php_functions.len(), 5);
        assert_eq!(
            state.native_php_functions[0].name,
            "oxphp_example_echo_upper"
        );
        assert_eq!(
            state.native_php_functions[1].name,
            "oxphp_example_string_info"
        );
        assert_eq!(
            state.native_php_functions[2].name,
            "oxphp_example_array_info"
        );
        assert_eq!(state.native_php_functions[3].name, "oxphp_example_make_dto");
        assert_eq!(
            state.native_php_functions[4].name,
            "oxphp_example_env_check"
        );

        // echo_upper: 1 required param
        assert_eq!(state.native_php_functions[0].params.len(), 1);
        assert_eq!(state.native_php_functions[0].params[0].name, "input");
        assert!(state.native_php_functions[0].params[0].required);
        assert_eq!(state.native_php_functions[0].return_type, PhpType::String);

        // make_dto: 1 optional param
        assert!(!state.native_php_functions[3].params[0].required);

        // env_check: no params
        assert_eq!(state.native_php_functions[4].params.len(), 0);
    }

    // ── echo_upper handler (mock FFI returns null → PhpError) ──

    #[test]
    fn test_example_echo_upper_handler_mock() {
        let (_plugin, state) = init_example_plugin();
        let handler = &state.native_php_functions[0].handler;

        // With mock FFI, arg_str returns null pointer → error
        let mut retval = [0u8; 16];
        let mut call = unsafe {
            NativeCall::new(
                std::ptr::null_mut(),
                1,
                retval.as_mut_ptr() as *mut std::os::raw::c_void,
                None,
                None,
            )
        };

        // Mock returns null string pointer → Custom error
        let result = handler.handle(&mut call);
        assert!(result.is_err());
    }

    // ── Complete handler (smoke test) ──

    #[test]
    fn test_example_complete_handler() {
        let handler = ExampleCompleteHandler;
        let view = PluginCompleteView::new(
            "req123",
            "GET",
            "/test",
            200,
            std::time::Duration::from_millis(42),
            "127.0.0.1:12345".parse().unwrap(),
            0,
            1024,
            &[],
        );
        handler.handle(&view); // should not panic
    }

    // ── Handler priorities ──

    #[test]
    fn test_example_handler_priorities() {
        let req_handler = ExampleRequestHandler {
            counter: Arc::new(AtomicU64::new(0)),
            verbose: false,
        };
        assert_eq!(req_handler.priority(), 50);

        let resp_handler = ExampleResponseHandler { verbose: false };
        assert_eq!(resp_handler.priority(), 50);

        let complete_handler = ExampleCompleteHandler;
        assert_eq!(complete_handler.priority(), 90);
    }
}
