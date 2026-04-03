pub mod connection_meta;
pub mod hooks;
pub mod php_sdk;
pub mod spans;
pub mod sql;

use std::cell::RefCell;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use opentelemetry::trace::{
    SpanBuilder, SpanId, SpanKind, Status, TraceFlags, TraceId, TracerProvider as _,
};
use opentelemetry::KeyValue;
use opentelemetry_sdk::trace::TracerProvider;

use crate::decorator::types::{
    AttributeTargets, DecoratorAction, DecoratorCallContext, DecoratorCallResult,
};
use crate::events::Priority;
use crate::plugin::handler::{
    PluginCompleteHandler, PluginCompleteView, PluginRequestActions, PluginRequestHandler,
    PluginRequestView,
};
use crate::plugin::{Plugin, PluginContext, PluginDeps, PluginError, PluginHealth};

use self::spans::SPAN_STACK;

// ---------------------------------------------------------------------------
// Thread-local to pass span local IDs between on_begin and on_end.
// The Decorator trait does not allow passing state directly, so we use a
// thread-local stack that mirrors the call nesting order.
// ---------------------------------------------------------------------------

thread_local! {
    static DECORATOR_SPAN_IDS: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };
}

/// Built-in decorator for the `#[OxPHP\Apm\Trace]` PHP attribute.
///
/// When a PHP developer annotates a function or method with this attribute,
/// the decorator automatically creates a span on entry and closes it on exit,
/// recording any exception as an error event.
struct TraceDecorator;

impl crate::decorator::Decorator for TraceDecorator {
    fn attribute_name(&self) -> &str {
        "OxPHP\\Apm\\Trace"
    }

    fn targets(&self) -> AttributeTargets {
        AttributeTargets::FUNCTION | AttributeTargets::METHOD
    }

    fn on_begin(&self, ctx: &DecoratorCallContext) -> DecoratorAction {
        let local_id =
            SPAN_STACK.with(|stack| stack.borrow_mut().push(ctx.target.to_string(), vec![]));

        DECORATOR_SPAN_IDS.with(|ids| ids.borrow_mut().push(local_id));

        DecoratorAction::Continue
    }

    fn on_end(&self, _ctx: &DecoratorCallContext, result: &DecoratorCallResult) {
        let local_id = DECORATOR_SPAN_IDS.with(|ids| ids.borrow_mut().pop());

        if let Some(local_id) = local_id {
            SPAN_STACK.with(|stack| {
                let mut stack = stack.borrow_mut();

                // If the decorated function threw, mark the span as error before closing.
                if !result.success {
                    if let Some(span) = stack.get_mut(local_id) {
                        span.status_code = 2; // Error
                        if let Some(ref exc_class) = result.exception_class {
                            span.events.push(spans::SpanEvent {
                                name: "exception".into(),
                                attributes: vec![("exception.type".into(), exc_class.clone())],
                                timestamp_us: spans::now_us(),
                            });
                        }
                    }
                }

                // Close the span (moves it from open to finished).
                stack.pop(local_id);
            });
        }
    }
}

/// APM configuration read from environment variables during init.
#[derive(Debug, Clone)]
struct ApmConfig {
    slow_query_ms: u64,
    db_capture_params: bool,
}

impl Default for ApmConfig {
    fn default() -> Self {
        Self {
            slow_query_ms: 100,
            db_capture_params: false,
        }
    }
}

/// APM plugin -- automatic performance monitoring and error capture.
///
/// Feature-gated behind `plugin-apm`. Depends on the `otel` plugin for
/// trace export. Reads `OTEL_APM_ENABLED` (or `APM_ENABLED`) to
/// enable/disable at runtime.
pub struct ApmPlugin {
    enabled: bool,
    config: ApmConfig,
}

impl Default for ApmPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ApmPlugin {
    pub fn new() -> Self {
        Self {
            enabled: false,
            config: ApmConfig::default(),
        }
    }
}

/// Request handler that resets the span stack per request.
///
/// Runs after trace context (-95) and otel (-80) handlers, so that
/// trace_id and span_id metadata are already present.
struct ApmRequestHandler;

impl PluginRequestHandler for ApmRequestHandler {
    fn handle(&self, _view: &PluginRequestView, _actions: &mut PluginRequestActions) {
        // NOTE: SpanStack reset and connection_meta::clear happen on the PHP
        // worker thread in execute_request() (executor/sapi.rs), not here.
        // This handler runs on Tokio thread — different TLS from PHP workers.
        // Kept for future use (e.g. extracting metadata for Tokio-side processing).
    }

    fn priority(&self) -> Priority {
        -70
    }
}

/// Completion handler that logs PHP errors and exports child spans to OTel.
struct ApmCompleteHandler {
    #[allow(dead_code)]
    slow_query_ms: u64,
    provider: Arc<OnceLock<TracerProvider>>,
}

impl PluginCompleteHandler for ApmCompleteHandler {
    fn handle(&self, view: &PluginCompleteView) {
        // ── Log PHP errors synchronously (lightweight, just tracing) ──
        if let Some(count_str) = view.metadata("oxphp.php_error_count") {
            let count: usize = count_str.parse().unwrap_or(0);
            for i in 0..count {
                let level = view
                    .metadata(&format!("oxphp.php_error.{i}.level"))
                    .unwrap_or("unknown");
                let error_type = view
                    .metadata(&format!("oxphp.php_error.{i}.type"))
                    .unwrap_or("unknown");
                let message = view
                    .metadata(&format!("oxphp.php_error.{i}.message"))
                    .unwrap_or("");
                let file = view
                    .metadata(&format!("oxphp.php_error.{i}.file"))
                    .unwrap_or("");
                let line = view
                    .metadata(&format!("oxphp.php_error.{i}.line"))
                    .unwrap_or("0");
                let stacktrace = view.metadata(&format!("oxphp.php_error.{i}.stacktrace"));

                tracing::info!(
                    plugin = "apm",
                    request_id = view.request_id,
                    php_error_level = level,
                    php_error_type = error_type,
                    php_file = file,
                    php_line = line,
                    has_stacktrace = stacktrace.is_some(),
                    "PHP error captured: {message}"
                );
            }
        }

        // ── Export child spans off the hot path via tokio::spawn ──
        let spans_json = match view.metadata("oxphp.apm_spans_json") {
            Some(json) if !json.is_empty() => json.to_string(),
            _ => return,
        };
        if self.provider.get().is_none() {
            return;
        }

        let provider = self.provider.clone();
        let request_id = view.request_id.to_string();

        tokio::spawn(async move {
            let provider = provider.get().unwrap(); // safe: checked above
            let tracer = provider.tracer("oxphp-apm");

            let parsed: Vec<serde_json::Value> = match serde_json::from_str(&spans_json) {
                Ok(v) => v,
                Err(e) => {
                    tracing::warn!(plugin = "apm", error = %e, "Failed to parse APM spans JSON");
                    return;
                }
            };

            let mut exported = 0u32;

            for span_val in &parsed {
                let tid = span_val["tid"].as_str().unwrap_or("");
                let sid = span_val["sid"].as_str().unwrap_or("");
                let pid = span_val["pid"].as_str().unwrap_or("");
                let name = span_val["n"].as_str().unwrap_or("unknown");
                let start_us = span_val["s"].as_u64().unwrap_or(0);
                let end_us = span_val["e"].as_u64().unwrap_or(0);
                let status_code = span_val["sc"].as_u64().unwrap_or(0) as u8;
                let status_msg = span_val["sm"].as_str();
                let leaked = span_val["l"].as_bool().unwrap_or(false);

                let trace_id = match TraceId::from_hex(tid) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let span_id = match SpanId::from_hex(sid) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let parent_span_id = SpanId::from_hex(pid).ok();

                let start_time = UNIX_EPOCH + std::time::Duration::from_micros(start_us);
                let end_time = UNIX_EPOCH + std::time::Duration::from_micros(end_us);

                let mut attributes: Vec<KeyValue> = Vec::new();
                if let Some(attrs) = span_val["a"].as_array() {
                    for pair in attrs {
                        if let (Some(k), Some(v)) = (pair[0].as_str(), pair[1].as_str()) {
                            attributes.push(KeyValue::new(k.to_string(), v.to_string()));
                        }
                    }
                }
                if leaked {
                    attributes.push(KeyValue::new("oxphp.span.leaked", true));
                }

                let status = match status_code {
                    2 => Status::error(status_msg.unwrap_or("error").to_string()),
                    1 => Status::Ok,
                    _ => Status::Unset,
                };

                use opentelemetry::trace::Span as _;

                let mut builder = SpanBuilder::from_name(name.to_string())
                    .with_trace_id(trace_id)
                    .with_span_id(span_id)
                    .with_kind(SpanKind::Internal)
                    .with_start_time(start_time)
                    .with_attributes(attributes)
                    .with_status(status);

                if let Some(parent_sid) = parent_span_id {
                    use opentelemetry::trace::{SpanContext, TraceContextExt};
                    let parent_ctx = SpanContext::new(
                        trace_id,
                        parent_sid,
                        TraceFlags::SAMPLED,
                        true,
                        Default::default(),
                    );
                    let parent_otel_ctx =
                        opentelemetry::Context::new().with_remote_span_context(parent_ctx);
                    // Use end_with_timestamp to set the correct end time.
                    // drop() would call end() which overwrites with SystemTime::now().
                    builder
                        .start_with_context(&tracer, &parent_otel_ctx)
                        .end_with_timestamp(end_time);
                } else {
                    builder.sampling_result = Some(opentelemetry::trace::SamplingResult {
                        decision: opentelemetry::trace::SamplingDecision::RecordAndSample,
                        attributes: Vec::new(),
                        trace_state: Default::default(),
                    });
                    builder.start(&tracer).end_with_timestamp(end_time);
                }
                exported += 1;
            }

            if exported > 0 {
                tracing::debug!(
                    plugin = "apm",
                    exported,
                    request_id,
                    "Exported APM child spans"
                );
            }
        });
    }

    fn priority(&self) -> Priority {
        -70
    }
}

impl Plugin for ApmPlugin {
    fn name(&self) -> &'static str {
        "apm"
    }

    fn version(&self) -> &'static str {
        "0.1.0"
    }

    fn dependencies(&self) -> PluginDeps {
        PluginDeps {
            required: vec!["otel"],
            ..Default::default()
        }
    }

    fn init(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        self.enabled = ctx
            .config("ENABLED")
            .or_else(|| ctx.config("OTEL_APM_ENABLED"))
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        if !self.enabled {
            tracing::info!(
                plugin = "apm",
                "APM plugin disabled (OTEL_APM_ENABLED != true)"
            );
            ctx.expose_config("enabled", false);

            // Register no-op PHP SDK functions even when disabled
            php_sdk::register_functions(ctx, false)?;

            // Register attribute class so #[Trace] doesn't fatal even when APM is off
            use crate::plugin::builders::attribute::{ATTR_TARGET_FUNCTION, ATTR_TARGET_METHOD};
            ctx.register_attribute("OxPHP\\Apm\\Trace")
                .target(ATTR_TARGET_FUNCTION | ATTR_TARGET_METHOD)
                .optional_param(
                    "name",
                    crate::plugin::types::PhpType::Nullable(Box::new(
                        crate::plugin::types::PhpType::String,
                    )),
                    crate::plugin::types::PhpValue::Null,
                )
                .build()?;

            return Ok(());
        }

        // Read additional config
        self.config.slow_query_ms = ctx
            .config("OTEL_APM_SLOW_QUERY_MS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(100);

        self.config.db_capture_params = ctx
            .config("OTEL_APM_DB_CAPTURE_PARAMS_ENABLED")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        // Expose config
        ctx.expose_config("enabled", true);
        ctx.expose_config("slow_query_ms", self.config.slow_query_ms);
        ctx.expose_config("db_capture_params", self.config.db_capture_params);

        php_sdk::register_functions(ctx, true)?;

        // Register the #[OxPHP\Apm\Trace] PHP attribute class
        {
            use crate::plugin::builders::attribute::{ATTR_TARGET_FUNCTION, ATTR_TARGET_METHOD};
            ctx.register_attribute("OxPHP\\Apm\\Trace")
                .target(ATTR_TARGET_FUNCTION | ATTR_TARGET_METHOD)
                .optional_param(
                    "name",
                    crate::plugin::types::PhpType::Nullable(Box::new(
                        crate::plugin::types::PhpType::String,
                    )),
                    crate::plugin::types::PhpValue::Null,
                )
                .build()?;
        }

        let provider: Arc<OnceLock<TracerProvider>> = ctx
            .service::<Arc<OnceLock<TracerProvider>>>("otel.provider")
            .cloned()
            .unwrap_or_else(|| Arc::new(OnceLock::new()));

        ctx.on_request(ApmRequestHandler);
        ctx.on_complete(ApmCompleteHandler {
            slow_query_ms: self.config.slow_query_ms,
            provider,
        });
        ctx.register_decorator(TraceDecorator);

        // Register internal PHP function hooks for automatic span creation.
        // register_all() populates the C bridge's pending list.
        // install_callbacks() sets the Rust before/after callbacks.
        // Actual hook installation happens per-thread during RINIT.
        let registered = hooks::register_all();
        hooks::install_callbacks();
        // Note: approved_count() would return 0 here because MINIT hasn't run yet.
        // Report the registration count instead — actual approval happens during MINIT.
        ctx.expose_config("hooks_registered", registered as u64);

        tracing::info!(
            plugin = "apm",
            slow_query_ms = self.config.slow_query_ms,
            db_capture_params = self.config.db_capture_params,
            hooks_registered = registered,
            "APM plugin initialized"
        );

        Ok(())
    }

    fn shutdown(&self) {
        if !self.enabled {
            return;
        }
        tracing::info!(plugin = "apm", "APM plugin shutdown complete");
    }

    fn health(&self) -> PluginHealth {
        PluginHealth::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventDispatcher;
    use crate::plugin::context::PluginDecoratorDef;
    use crate::plugin::handler::{PluginInternalHandler, PluginMetricsCollector};
    use crate::plugin::php::PluginNativeFunctionDef;
    use std::collections::HashMap;

    fn init_apm_plugin(plugin: &mut ApmPlugin) -> HashMap<String, serde_json::Value> {
        let mut dispatcher = EventDispatcher::new();
        let mut services: HashMap<String, Box<dyn std::any::Any + Send + Sync>> = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics_collectors: Vec<Box<dyn PluginMetricsCollector>> = Vec::new();
        let mut internal_routes: HashMap<String, Box<dyn PluginInternalHandler>> = HashMap::new();
        let mut native_php_functions: Vec<PluginNativeFunctionDef> = Vec::new();
        let mut decorators: Vec<PluginDecoratorDef> = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = PluginContext::new(
            "apm".into(),
            "__oxp_apm_".into(),
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics_collectors,
            &mut internal_routes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );
        plugin.init(&mut ctx).unwrap();
        drop(ctx);
        config_values
    }

    #[test]
    fn test_apm_plugin_disabled_by_default() {
        std::env::remove_var("APM_ENABLED");
        std::env::remove_var("OTEL_APM_ENABLED");
        let mut plugin = ApmPlugin::new();
        let config = init_apm_plugin(&mut plugin);

        assert_eq!(plugin.name(), "apm");
        assert_eq!(plugin.version(), "0.1.0");
        assert!(!plugin.enabled);
        assert_eq!(config.get("enabled"), Some(&serde_json::json!(false)));
        assert_eq!(plugin.health(), PluginHealth::Ok);
    }

    #[test]
    fn test_apm_plugin_depends_on_otel() {
        let plugin = ApmPlugin::new();
        let deps = plugin.dependencies();
        assert_eq!(deps.required, vec!["otel"]);
        assert!(deps.optional.is_empty());
        assert!(deps.services.is_empty());
    }

    #[test]
    fn test_apm_plugin_health_ok() {
        let plugin = ApmPlugin::new();
        assert_eq!(plugin.health(), PluginHealth::Ok);
    }

    #[test]
    fn test_apm_plugin_name_and_version() {
        let plugin = ApmPlugin::new();
        assert_eq!(plugin.name(), "apm");
        assert_eq!(plugin.version(), "0.1.0");
    }

    #[test]
    fn test_apm_plugin_enabled_via_env() {
        std::env::set_var("OTEL_APM_ENABLED", "true");
        let mut plugin = ApmPlugin::new();
        let config = init_apm_plugin(&mut plugin);

        assert!(plugin.enabled);
        assert_eq!(config.get("enabled"), Some(&serde_json::json!(true)));
        std::env::remove_var("OTEL_APM_ENABLED");
    }

    #[test]
    fn test_apm_plugin_enabled_via_prefixed_env() {
        std::env::remove_var("OTEL_APM_ENABLED");
        std::env::set_var("APM_ENABLED", "1");
        let mut plugin = ApmPlugin::new();
        let config = init_apm_plugin(&mut plugin);

        assert!(plugin.enabled);
        assert_eq!(config.get("enabled"), Some(&serde_json::json!(true)));
        std::env::remove_var("APM_ENABLED");
    }

    #[test]
    fn test_apm_plugin_shutdown_disabled() {
        let plugin = ApmPlugin::new();
        plugin.shutdown(); // should not panic
    }

    #[test]
    fn test_apm_plugin_shutdown_enabled() {
        std::env::set_var("OTEL_APM_ENABLED", "true");
        let mut plugin = ApmPlugin::new();
        init_apm_plugin(&mut plugin);
        plugin.shutdown(); // should not panic
        std::env::remove_var("OTEL_APM_ENABLED");
    }

    #[test]
    fn test_apm_plugin_default_trait() {
        let plugin = ApmPlugin::default();
        assert_eq!(plugin.name(), "apm");
        assert!(!plugin.enabled);
    }

    #[test]
    fn test_custom_config() {
        std::env::set_var("OTEL_APM_ENABLED", "true");
        std::env::set_var("OTEL_APM_SLOW_QUERY_MS", "250");
        std::env::set_var("OTEL_APM_DB_CAPTURE_PARAMS_ENABLED", "true");

        let mut plugin = ApmPlugin::new();
        let config = init_apm_plugin(&mut plugin);

        assert_eq!(plugin.config.slow_query_ms, 250);
        assert!(plugin.config.db_capture_params);
        assert_eq!(config.get("slow_query_ms"), Some(&serde_json::json!(250)));
        assert_eq!(
            config.get("db_capture_params"),
            Some(&serde_json::json!(true))
        );

        std::env::remove_var("OTEL_APM_ENABLED");
        std::env::remove_var("OTEL_APM_SLOW_QUERY_MS");
        std::env::remove_var("OTEL_APM_DB_CAPTURE_PARAMS_ENABLED");
    }

    #[test]
    fn test_request_handler_is_noop() {
        // ApmRequestHandler is a no-op on Tokio thread.
        // Actual span stack reset happens on PHP worker thread in execute_request().
        let handler = ApmRequestHandler;
        let method = http::Method::GET;
        let uri: http::Uri = "/test".parse().unwrap();
        let addr: std::net::SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let headers = http::HeaderMap::new();
        let cookies = crate::plugin::cookies::PluginCookies { cookies: vec![] };
        let view = PluginRequestView::new(&method, &uri, addr, "req-1", &headers, cookies, &[]);
        let mut actions = PluginRequestActions::new();
        handler.handle(&view, &mut actions); // should not panic
    }

    #[test]
    fn test_request_handler_priority() {
        let handler = ApmRequestHandler;
        assert_eq!(handler.priority(), -70);
    }

    #[test]
    fn test_complete_handler_priority() {
        let handler = ApmCompleteHandler {
            slow_query_ms: 100,
            provider: Arc::new(OnceLock::new()),
        };
        assert_eq!(handler.priority(), -70);
    }

    #[test]
    fn test_complete_handler_with_spans_json() {
        // Simulate spans JSON that would come from PHP worker thread
        let spans_json = r#"[{"tid":"aaaaaaaaaaaaaaaabbbbbbbbbbbbbbbb","sid":"1111111111111111","pid":"2222222222222222","n":"test.span","s":1700000000000000,"e":1700000000000500,"a":[["key","val"]],"ev":[],"sc":0,"sm":null,"l":false}]"#;

        let handler = ApmCompleteHandler {
            slow_query_ms: 100,
            provider: Arc::new(OnceLock::new()),
        };

        let addr: std::net::SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let metadata = vec![("oxphp.apm_spans_json".to_string(), spans_json.to_string())];
        let view = PluginCompleteView::new(
            "req-1",
            "GET",
            "/test",
            200,
            std::time::Duration::from_millis(10),
            addr,
            0,
            0,
            &metadata,
        );

        handler.handle(&view); // should not panic even without TracerProvider
    }

    #[test]
    fn test_default_config_values() {
        std::env::set_var("OTEL_APM_ENABLED", "true");
        std::env::remove_var("OTEL_APM_SLOW_QUERY_MS");
        std::env::remove_var("OTEL_APM_DB_CAPTURE_PARAMS_ENABLED");

        let mut plugin = ApmPlugin::new();
        let config = init_apm_plugin(&mut plugin);

        assert_eq!(plugin.config.slow_query_ms, 100);
        assert!(!plugin.config.db_capture_params);
        assert_eq!(config.get("slow_query_ms"), Some(&serde_json::json!(100)));
        assert_eq!(
            config.get("db_capture_params"),
            Some(&serde_json::json!(false))
        );

        std::env::remove_var("OTEL_APM_ENABLED");
    }

    // -----------------------------------------------------------------------
    // TraceDecorator tests
    // -----------------------------------------------------------------------

    use crate::decorator::Decorator;

    fn make_call_context(target: &str) -> DecoratorCallContext {
        DecoratorCallContext {
            target: Arc::from(target),
            class: None,
            method: None,
            function: Some(Arc::from(target)),
            object_id: 0,
            request_id: "req-test".into(),
            trace_id: "trace-test".into(),
            timestamp_ns: 1000,
        }
    }

    #[test]
    fn test_trace_decorator_attribute_name() {
        let decorator = TraceDecorator;
        assert_eq!(decorator.attribute_name(), "OxPHP\\Apm\\Trace");
    }

    #[test]
    fn test_trace_decorator_targets() {
        let decorator = TraceDecorator;
        let targets = decorator.targets();
        assert!(targets.contains(AttributeTargets::FUNCTION));
        assert!(targets.contains(AttributeTargets::METHOD));
        assert!(!targets.contains(AttributeTargets::CLASS));
    }

    #[test]
    fn test_trace_decorator_on_begin_creates_span() {
        // Reset span stack and decorator span IDs
        SPAN_STACK.with(|s| s.borrow_mut().reset("trace-1".into(), "root-1".into()));
        DECORATOR_SPAN_IDS.with(|ids| ids.borrow_mut().clear());

        let decorator = TraceDecorator;
        let ctx = make_call_context("App\\Service::findById");

        let action = decorator.on_begin(&ctx);
        assert_eq!(action, DecoratorAction::Continue);

        // Verify a span was pushed onto the stack
        SPAN_STACK.with(|s| {
            let stack = s.borrow();
            assert_eq!(stack.open_count(), 1);
            let current = stack.current().unwrap();
            assert_eq!(current.name, "App\\Service::findById");
        });

        // Verify local_id was saved in thread-local
        DECORATOR_SPAN_IDS.with(|ids| {
            assert_eq!(ids.borrow().len(), 1);
        });
    }

    #[test]
    fn test_trace_decorator_on_end_closes_span() {
        // Reset span stack and decorator span IDs
        SPAN_STACK.with(|s| s.borrow_mut().reset("trace-2".into(), "root-2".into()));
        DECORATOR_SPAN_IDS.with(|ids| ids.borrow_mut().clear());

        let decorator = TraceDecorator;
        let ctx = make_call_context("my_function");

        // Begin
        decorator.on_begin(&ctx);
        assert_eq!(SPAN_STACK.with(|s| s.borrow().open_count()), 1);

        // End (success)
        let result = DecoratorCallResult {
            success: true,
            elapsed_ns: 5_000_000,
            exception_class: None,
        };
        decorator.on_end(&ctx, &result);

        // Span should be closed (moved to finished)
        SPAN_STACK.with(|s| {
            let stack = s.borrow();
            assert_eq!(stack.open_count(), 0);
            assert_eq!(stack.finished_count(), 1);
        });

        // Verify the finished span
        SPAN_STACK.with(|s| {
            let mut stack = s.borrow_mut();
            let finished = stack.take_finished();
            assert_eq!(finished.len(), 1);
            assert_eq!(finished[0].name, "my_function");
            assert_eq!(finished[0].status_code, 0); // Unset (success)
            assert!(finished[0].events.is_empty());
            assert!(!finished[0].leaked);
        });

        // Decorator span IDs should be empty
        DECORATOR_SPAN_IDS.with(|ids| {
            assert!(ids.borrow().is_empty());
        });
    }

    #[test]
    fn test_trace_decorator_on_end_records_exception() {
        // Reset span stack and decorator span IDs
        SPAN_STACK.with(|s| s.borrow_mut().reset("trace-3".into(), "root-3".into()));
        DECORATOR_SPAN_IDS.with(|ids| ids.borrow_mut().clear());

        let decorator = TraceDecorator;
        let ctx = make_call_context("App\\PaymentService::charge");

        // Begin
        decorator.on_begin(&ctx);

        // End with exception
        let result = DecoratorCallResult {
            success: false,
            elapsed_ns: 1_000,
            exception_class: Some("RuntimeException".into()),
        };
        decorator.on_end(&ctx, &result);

        // Span should be closed with error status
        SPAN_STACK.with(|s| {
            let mut stack = s.borrow_mut();
            let finished = stack.take_finished();
            assert_eq!(finished.len(), 1);
            assert_eq!(finished[0].name, "App\\PaymentService::charge");
            assert_eq!(finished[0].status_code, 2); // Error

            // Should have an exception event
            assert_eq!(finished[0].events.len(), 1);
            assert_eq!(finished[0].events[0].name, "exception");
            assert_eq!(finished[0].events[0].attributes.len(), 1);
            assert_eq!(finished[0].events[0].attributes[0].0, "exception.type");
            assert_eq!(finished[0].events[0].attributes[0].1, "RuntimeException");
        });
    }
}
