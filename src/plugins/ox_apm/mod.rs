pub mod connection_meta;
pub mod hooks;
pub mod php_sdk;
pub mod sql;

use std::borrow::Cow;
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::UNIX_EPOCH;

use opentelemetry::trace::{
    SpanBuilder, SpanId, SpanKind, Status, TraceFlags, TraceId, TracerProvider as _,
};
use opentelemetry::KeyValue;
use opentelemetry_sdk::trace::SdkTracerProvider;

use crate::decorator::types::{
    AttributeTargets, DecoratorAction, DecoratorCallContext, DecoratorCallResult,
};
use crate::events::Priority;
use crate::plugin::handler::{
    PluginCompleteHandler, PluginCompleteView, PluginRequestActions, PluginRequestHandler,
    PluginRequestView,
};
use crate::plugin::{Plugin, PluginContext, PluginDeps, PluginError, PluginHealth};

use crate::profiling::{now_ns, SpanEvent, SpanEventKind, PROFILING_CONTEXT};

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
        let local_id = PROFILING_CONTEXT.with(|stack| {
            stack
                .borrow_mut()
                .push(Arc::from(ctx.target.as_ref()), vec![])
        });

        DECORATOR_SPAN_IDS.with(|ids| ids.borrow_mut().push(local_id));

        DecoratorAction::Continue
    }

    fn on_end(&self, _ctx: &DecoratorCallContext, result: &DecoratorCallResult) {
        let local_id = DECORATOR_SPAN_IDS.with(|ids| ids.borrow_mut().pop());

        if let Some(local_id) = local_id {
            PROFILING_CONTEXT.with(|stack| {
                let mut stack = stack.borrow_mut();

                // If the decorated function threw, mark the span as error before closing.
                if !result.success {
                    if let Some(span) = stack.get_mut(local_id) {
                        span.status_code = 2; // Error
                        if let Some(exc_class) = result.exception_class.as_deref() {
                            push_exception_event(
                                span,
                                exc_class,
                                result.exception_message.as_deref(),
                                result.exception_stacktrace.as_deref(),
                                MESSAGE_MAX_BYTES.load(Ordering::Relaxed),
                                STACKTRACE_MAX_BYTES.load(Ordering::Relaxed),
                            );
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
        // NOTE: ProfilingContext reset and connection_meta::clear happen on the PHP
        // worker thread in execute_request() (executor/sapi.rs), not here.
        // This handler runs on Tokio thread — different TLS from PHP workers.
        // Kept for future use (e.g. extracting metadata for Tokio-side processing).
    }

    fn priority(&self) -> Priority {
        -70
    }
}

/// Default cap for the `exception.stacktrace` attribute (bytes).
const DEFAULT_STACKTRACE_MAX_BYTES: usize = 8192;

/// Default cap for the `exception.message` attribute (bytes). 4096 matches the
/// per-attribute value limit New Relic applies on ingest, so a larger value
/// would be truncated downstream anyway; a real message rarely exceeds a few
/// hundred bytes.
const DEFAULT_MESSAGE_MAX_BYTES: usize = 4096;

/// Runtime cap (bytes) for `exception.stacktrace`, set from
/// `OTEL_APM_STACKTRACE_MAX_BYTES` in `ApmPlugin::init`. `0` disables
/// truncation. Read on the exception path by both the `#[Trace]` decorator and
/// `oxphp_apm_error`.
static STACKTRACE_MAX_BYTES: AtomicUsize = AtomicUsize::new(DEFAULT_STACKTRACE_MAX_BYTES);

/// Runtime cap (bytes) for `exception.message`, set from
/// `OTEL_APM_MESSAGE_MAX_BYTES` in `ApmPlugin::init`. `0` disables truncation.
static MESSAGE_MAX_BYTES: AtomicUsize = AtomicUsize::new(DEFAULT_MESSAGE_MAX_BYTES);

/// Truncate an exception attribute string to at most `max_bytes`, appending a
/// marker when it was cut. `max_bytes == 0` disables truncation. Cuts on a
/// UTF-8 char boundary. For a stacktrace (`getTraceAsString()` is top-down),
/// the root frame (`#0`, the throw site) is preserved and only the tail
/// (`{main}`-ward) is dropped. The result is always `<= max_bytes`: when even
/// the marker would not fit (`max_bytes <= MARKER.len()`), the content is
/// hard-cut with no marker.
fn truncate_attr(s: &str, max_bytes: usize) -> Cow<'_, str> {
    if max_bytes == 0 || s.len() <= max_bytes {
        return Cow::Borrowed(s);
    }
    const MARKER: &str = "…(truncated)";
    let with_marker = max_bytes > MARKER.len();
    let budget = if with_marker {
        max_bytes - MARKER.len()
    } else {
        max_bytes
    };
    let mut end = budget.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    let mut out = String::with_capacity(end + MARKER.len());
    out.push_str(&s[..end]);
    if with_marker {
        out.push_str(MARKER);
    }
    Cow::Owned(out)
}

/// Push an OTel-semconv `exception` span event. Each attribute is omitted when
/// empty (`exception.type` too, so a bare string reason can be recorded as
/// message-only). The message is truncated to `message_max` and the stacktrace
/// to `stacktrace_max` — the caps are passed in (not read from the globals)
/// so callers stay in control and tests need not mutate process-wide state.
///
/// Called once per decorated frame the exception unwinds through (each span in
/// the error chain records its own exception event, per OTel). The per-attribute
/// caps bound each event, but there is deliberately no *aggregate* per-trace cap:
/// a pathologically deep recursive `#[Trace]` chain (hundreds of frames) can thus
/// produce a large trace payload. Kept stateless on purpose — a per-trace dedup
/// or byte budget would add per-worker state that fibers multiplexed onto one
/// worker would corrupt, for a case that is rare in practice.
fn push_exception_event(
    span: &mut crate::profiling::PendingSpan,
    exc_type: &str,
    message: Option<&str>,
    stacktrace: Option<&str>,
    message_max: usize,
    stacktrace_max: usize,
) {
    let mut attributes: Vec<(Arc<str>, Arc<str>)> = Vec::with_capacity(3);
    if !exc_type.is_empty() {
        // An anonymous class name is "<parent>@anonymous\0<file>:<line>$<hash>"
        // with an embedded NUL. It arrives length-delimited (so it is not
        // truncated at the NUL on the way in), but a NUL is valid UTF-8 and
        // would otherwise ride into the attribute and truncate the type again in
        // any NUL-terminating downstream — strip it here so the type stays clean
        // and distinct. No-op for ordinary class names, which never hold a NUL.
        let ty: Cow<str> = if exc_type.contains('\0') {
            Cow::Owned(exc_type.replace('\0', ""))
        } else {
            Cow::Borrowed(exc_type)
        };
        attributes.push((Arc::from("exception.type"), Arc::from(ty.as_ref())));
    }
    if let Some(m) = message.filter(|s| !s.is_empty()) {
        attributes.push((
            Arc::from("exception.message"),
            Arc::from(truncate_attr(m, message_max).as_ref()),
        ));
    }
    if let Some(t) = stacktrace.filter(|s| !s.is_empty()) {
        attributes.push((
            Arc::from("exception.stacktrace"),
            Arc::from(truncate_attr(t, stacktrace_max).as_ref()),
        ));
    }
    span.events.push(SpanEvent {
        name: "exception".into(),
        attributes,
        timestamp_ns: now_ns(),
        kind: SpanEventKind::Exception,
    });
}

/// Parse a byte-cap env var (falling back to `default`), publish it to `cell`
/// (the single source of truth read by the decorator and SDK on the exception
/// path), and expose it on the internal config endpoint. Shared by the two
/// exception-attribute caps so their parsing cannot drift apart.
fn read_cap(
    ctx: &mut PluginContext,
    env: &str,
    expose_key: &str,
    cell: &AtomicUsize,
    default: usize,
) {
    let value = match ctx.config(env).as_deref().map(str::trim) {
        // Unset, or set-but-blank (e.g. `OTEL_APM_MESSAGE_MAX_BYTES=` in a
        // compose file): treat as "use the default", silently — matching the
        // sibling caps (SLOW_QUERY_MS, TLS_MIN_VERSION).
        None | Some("") => default,
        Some(trimmed) => match trimmed.parse() {
            Ok(v) => v,
            Err(_) => {
                // A typo'd cap (e.g. "8k", "0x2000") would otherwise silently
                // apply the default, leaving the operator to wonder why their
                // exception.message is truncated. Surface it.
                tracing::warn!(
                    plugin = "apm",
                    env,
                    value = %trimmed,
                    default,
                    "invalid byte cap; expected a non-negative integer, using default"
                );
                default
            }
        },
    };
    cell.store(value, Ordering::Relaxed);
    ctx.expose_config(expose_key, value as u64);
}

/// Stable string tag for a span event's semantic kind. Emitted as the
/// `oxphp.event.kind` attribute on the exported OTel event so backends
/// (Jaeger / Tempo / Grafana) can distinguish `slow` / `memory_spike` /
/// `mark` / `exception` events that would otherwise differ only by name.
fn event_kind_str(kind: SpanEventKind) -> &'static str {
    match kind {
        SpanEventKind::Mark => "mark",
        SpanEventKind::Sql => "sql",
        SpanEventKind::Http => "http",
        SpanEventKind::Exception => "exception",
        SpanEventKind::Slow => "slow",
        SpanEventKind::MemorySpike => "memory_spike",
        SpanEventKind::Alloc => "alloc",
        SpanEventKind::Custom => "custom",
    }
}

/// Map a profiling [`SpanEvent`] to an OpenTelemetry span event. The
/// event's `kind` is preserved as the `oxphp.event.kind` attribute since
/// OTel events carry no kind field of their own; the timestamp is the
/// Unix-epoch nanosecond clock the profiler records.
fn map_span_event(ev: &SpanEvent) -> opentelemetry::trace::Event {
    let mut attributes: Vec<KeyValue> = ev
        .attributes
        .iter()
        .map(|(k, v)| KeyValue::new(k.clone(), v.clone()))
        .collect();
    attributes.push(KeyValue::new("oxphp.event.kind", event_kind_str(ev.kind)));
    let timestamp = UNIX_EPOCH + std::time::Duration::from_nanos(ev.timestamp_ns);
    opentelemetry::trace::Event::new(ev.name.clone(), timestamp, attributes, 0)
}

/// Completion handler that logs PHP errors and exports child spans to OTel.
struct ApmCompleteHandler {
    #[allow(dead_code)]
    slow_query_ms: u64,
    provider: Arc<OnceLock<SdkTracerProvider>>,
}

impl PluginCompleteHandler for ApmCompleteHandler {
    fn handle(&self, view: &PluginCompleteView) {
        // ── Log PHP errors synchronously (lightweight, just tracing) ──
        for err in view.php_errors {
            tracing::info!(
                plugin = "apm",
                request_id = view.request_id,
                php_error_level = err.level,
                php_error_type = err.error_type,
                php_file = %err.file,
                php_line = err.line,
                has_stacktrace = err.stacktrace.is_some(),
                "PHP error captured: {}", err.message
            );
        }

        // ── Export child spans off the hot path via tokio::spawn ──
        let tree = match view.profile_tree {
            Some(t) if !t.is_empty() => Arc::clone(t),
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

            let mut exported = 0u32;

            for span in tree.finished_spans() {
                let trace_id = match TraceId::from_hex(&span.trace_id) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let span_id = match SpanId::from_hex(&span.span_id) {
                    Ok(id) => id,
                    Err(_) => continue,
                };
                let parent_span_id = SpanId::from_hex(&span.parent_span_id).ok();

                let start_time = UNIX_EPOCH + std::time::Duration::from_nanos(span.start_ns);
                let end_time = UNIX_EPOCH + std::time::Duration::from_nanos(span.end_ns);

                let mut attributes: Vec<KeyValue> = span
                    .attributes
                    .iter()
                    .map(|(k, v)| KeyValue::new(k.clone(), v.clone()))
                    .collect();
                if span.leaked {
                    attributes.push(KeyValue::new("oxphp.span.leaked", true));
                }

                let status = match span.status_code {
                    2 => Status::error(
                        span.status_message
                            .as_deref()
                            .unwrap_or("error")
                            .to_string(),
                    ),
                    1 => Status::Ok,
                    _ => Status::Unset,
                };

                // Span events (slow / memory_spike / mark / exception /
                // custom) carry the threshold and label data attached by
                // the profiler decorators and the APM hooks. Map them to
                // OTel span events so they surface in Jaeger / Tempo /
                // Grafana instead of being dropped on export.
                let events: Vec<opentelemetry::trace::Event> =
                    span.events.iter().map(map_span_event).collect();

                use opentelemetry::trace::Span as _;

                let mut builder = SpanBuilder::from_name(span.name.to_string())
                    .with_trace_id(trace_id)
                    .with_span_id(span_id)
                    .with_kind(SpanKind::Internal)
                    .with_start_time(start_time)
                    .with_attributes(attributes)
                    .with_status(status);
                if !events.is_empty() {
                    builder = builder.with_events(events);
                }

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
        let enabled_raw = ctx
            .config("ENABLED")
            .or_else(|| ctx.config("OTEL_APM_ENABLED"));
        self.enabled =
            crate::config::parse_bool_opt("OTEL_APM_ENABLED", enabled_raw.as_deref(), false)
                .map_err(|e| PluginError::Config(e.to_string()))?;
        let db_capture_raw = ctx.config("OTEL_APM_DB_CAPTURE_PARAMS_ENABLED");
        let db_capture_params = crate::config::parse_bool_opt(
            "OTEL_APM_DB_CAPTURE_PARAMS_ENABLED",
            db_capture_raw.as_deref(),
            false,
        )
        .map_err(|e| PluginError::Config(e.to_string()))?;

        if !self.enabled {
            tracing::info!(
                plugin = "apm",
                "APM plugin disabled (OTEL_APM_ENABLED is falsy or unset)"
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

        self.config.db_capture_params = db_capture_params;

        read_cap(
            ctx,
            "OTEL_APM_STACKTRACE_MAX_BYTES",
            "stacktrace_max_bytes",
            &STACKTRACE_MAX_BYTES,
            DEFAULT_STACKTRACE_MAX_BYTES,
        );
        read_cap(
            ctx,
            "OTEL_APM_MESSAGE_MAX_BYTES",
            "message_max_bytes",
            &MESSAGE_MAX_BYTES,
            DEFAULT_MESSAGE_MAX_BYTES,
        );

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

        let provider: Arc<OnceLock<SdkTracerProvider>> = ctx
            .service::<Arc<OnceLock<SdkTracerProvider>>>("otel.provider")
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

    fn shutdown(&mut self) {
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
        let mut internal_route_prefixes: Vec<(String, Box<dyn PluginInternalHandler>)> = Vec::new();
        let mut native_php_functions: Vec<PluginNativeFunctionDef> = Vec::new();
        let mut decorators: Vec<PluginDecoratorDef> = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();
        let mut core_flags = HashMap::new();

        let mut ctx = PluginContext::new(
            "apm".into(),
            "__oxp_apm_".into(),
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics_collectors,
            &mut internal_routes,
            &mut internal_route_prefixes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
            &mut core_flags,
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
        let mut plugin = ApmPlugin::new();
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
    fn test_complete_handler_with_profile_tree() {
        use crate::profiling::{FinishedSpan, ProfilingMode, SpanTree};

        let finished = vec![FinishedSpan {
            local_id: 1,
            trace_id: "aaaaaaaaaaaaaaaabbbbbbbbbbbbbbbb".into(),
            span_id: "1111111111111111".into(),
            parent_span_id: "2222222222222222".into(),
            name: "test.span".into(),
            start_ns: 1700000000000000,
            end_ns: 1700000000000500,
            cpu_ns: 0,
            mem_enter: 0,
            mem_exit: 0,
            mem_peak: 0,
            attributes: vec![("key".into(), "val".into())],
            events: vec![],
            status_code: 0,
            status_message: None,
            leaked: false,
        }];
        let tree = Arc::new(SpanTree {
            finished,
            trace_id: "aaaaaaaaaaaaaaaabbbbbbbbbbbbbbbb".into(),
            root_span_id: "2222222222222222".into(),
            mode: ProfilingMode::ApmOnly,
        });

        let handler = ApmCompleteHandler {
            slow_query_ms: 100,
            provider: Arc::new(OnceLock::new()),
        };

        let addr: std::net::SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let view = PluginCompleteView::new(
            "req-1",
            "GET",
            "/test",
            200,
            std::time::Duration::from_millis(10),
            addr,
            0,
            0,
            &[],
            &[],
            Some(&tree),
            None,
            None,
        );

        handler.handle(&view); // should not panic even without TracerProvider
    }

    #[test]
    fn map_span_event_preserves_name_attributes_kind_and_timestamp() {
        use crate::profiling::SpanEvent;

        let ev = SpanEvent {
            name: "slow".into(),
            attributes: vec![
                (Arc::from("threshold_ms"), Arc::from("1")),
                (Arc::from("elapsed_ms"), Arc::from("150")),
            ],
            timestamp_ns: 1_700_000_000_000_000_000,
            kind: SpanEventKind::Slow,
        };

        let otel = map_span_event(&ev);

        assert_eq!(otel.name.as_ref(), "slow");
        // threshold_ms + elapsed_ms + the synthesised oxphp.event.kind.
        assert_eq!(otel.attributes.len(), 3);

        let attr = |key: &str| {
            otel.attributes
                .iter()
                .find(|kv| kv.key.as_str() == key)
                .map(|kv| kv.value.as_str().into_owned())
        };
        assert_eq!(attr("threshold_ms").as_deref(), Some("1"));
        assert_eq!(attr("elapsed_ms").as_deref(), Some("150"));
        assert_eq!(attr("oxphp.event.kind").as_deref(), Some("slow"));

        assert_eq!(
            otel.timestamp,
            UNIX_EPOCH + std::time::Duration::from_nanos(1_700_000_000_000_000_000)
        );
    }

    #[test]
    fn event_kind_str_covers_every_variant() {
        assert_eq!(event_kind_str(SpanEventKind::Mark), "mark");
        assert_eq!(event_kind_str(SpanEventKind::Sql), "sql");
        assert_eq!(event_kind_str(SpanEventKind::Http), "http");
        assert_eq!(event_kind_str(SpanEventKind::Exception), "exception");
        assert_eq!(event_kind_str(SpanEventKind::Slow), "slow");
        assert_eq!(event_kind_str(SpanEventKind::MemorySpike), "memory_spike");
        assert_eq!(event_kind_str(SpanEventKind::Alloc), "alloc");
        assert_eq!(event_kind_str(SpanEventKind::Custom), "custom");
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
        PROFILING_CONTEXT.with(|s| {
            s.borrow_mut().reset(
                crate::profiling::ProfilingMode::ApmOnly,
                "trace-1".into(),
                "root-1".into(),
            )
        });
        DECORATOR_SPAN_IDS.with(|ids| ids.borrow_mut().clear());

        let decorator = TraceDecorator;
        let ctx = make_call_context("App\\Service::findById");

        let action = decorator.on_begin(&ctx);
        assert_eq!(action, DecoratorAction::Continue);

        // Verify a span was pushed onto the stack
        PROFILING_CONTEXT.with(|s| {
            let stack = s.borrow();
            assert_eq!(stack.open_count(), 1);
            let current = stack.current().unwrap();
            assert_eq!(current.name.as_ref(), "App\\Service::findById");
        });

        // Verify local_id was saved in thread-local
        DECORATOR_SPAN_IDS.with(|ids| {
            assert_eq!(ids.borrow().len(), 1);
        });
    }

    #[test]
    fn test_trace_decorator_on_end_closes_span() {
        // Reset span stack and decorator span IDs
        PROFILING_CONTEXT.with(|s| {
            s.borrow_mut().reset(
                crate::profiling::ProfilingMode::ApmOnly,
                "trace-2".into(),
                "root-2".into(),
            )
        });
        DECORATOR_SPAN_IDS.with(|ids| ids.borrow_mut().clear());

        let decorator = TraceDecorator;
        let ctx = make_call_context("my_function");

        // Begin
        decorator.on_begin(&ctx);
        assert_eq!(PROFILING_CONTEXT.with(|s| s.borrow().open_count()), 1);

        // End (success)
        let result = DecoratorCallResult {
            success: true,
            elapsed_ns: 5_000_000,
            exception_class: None,
            exception_message: None,
            exception_stacktrace: None,
        };
        decorator.on_end(&ctx, &result);

        // Span should be closed (moved to finished)
        PROFILING_CONTEXT.with(|s| {
            let stack = s.borrow();
            assert_eq!(stack.open_count(), 0);
            assert_eq!(stack.finished_count(), 1);
        });

        // Verify the finished span
        PROFILING_CONTEXT.with(|s| {
            let mut stack = s.borrow_mut();
            let finished = stack.take_finished();
            assert_eq!(finished.len(), 1);
            assert_eq!(finished[0].name.as_ref(), "my_function");
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
        PROFILING_CONTEXT.with(|s| {
            s.borrow_mut().reset(
                crate::profiling::ProfilingMode::ApmOnly,
                "trace-3".into(),
                "root-3".into(),
            )
        });
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
            exception_message: Some("connection refused".into()),
            exception_stacktrace: Some("#0 /app/Db.php(9): connect()\n#1 {main}".into()),
        };
        decorator.on_end(&ctx, &result);

        // Span should be closed with error status
        PROFILING_CONTEXT.with(|s| {
            let mut stack = s.borrow_mut();
            let finished = stack.take_finished();
            assert_eq!(finished.len(), 1);
            assert_eq!(finished[0].name.as_ref(), "App\\PaymentService::charge");
            assert_eq!(finished[0].status_code, 2); // Error

            // Should have an exception event with full OTel data
            assert_eq!(finished[0].events.len(), 1);
            let ev = &finished[0].events[0];
            assert_eq!(ev.name, "exception");
            assert_eq!(ev.attributes.len(), 3);
            assert_eq!(ev.attributes[0].0.as_ref(), "exception.type");
            assert_eq!(ev.attributes[0].1.as_ref(), "RuntimeException");
            assert_eq!(ev.attributes[1].0.as_ref(), "exception.message");
            assert_eq!(ev.attributes[1].1.as_ref(), "connection refused");
            assert_eq!(ev.attributes[2].0.as_ref(), "exception.stacktrace");
            assert!(ev.attributes[2].1.as_ref().contains("#0 /app/Db.php(9)"));
        });
    }

    #[test]
    fn truncate_short_is_borrowed() {
        let s = "#0 /app/x.php(1): f()\n#1 {main}";
        match truncate_attr(s, 8192) {
            Cow::Borrowed(b) => assert_eq!(b, s),
            Cow::Owned(_) => panic!("short trace must not be copied"),
        }
    }

    #[test]
    fn truncate_zero_disables() {
        let s = "a".repeat(100_000);
        assert!(matches!(truncate_attr(&s, 0), Cow::Borrowed(_)));
    }

    #[test]
    fn truncate_long_marks_and_bounds() {
        let s = "x".repeat(20_000);
        let out = truncate_attr(&s, 8192);
        assert!(out.ends_with("…(truncated)"));
        assert!(out.len() <= 8192, "len was {}", out.len());
    }

    #[test]
    fn truncate_respects_utf8_boundary() {
        // "é" is 2 bytes; a naive byte cut would split it and be invalid UTF-8.
        let s = "é".repeat(100); // 200 bytes
        let out = truncate_attr(&s, 50);
        assert!(out.ends_with("…(truncated)"));
        assert!(out.len() <= 50);
    }

    #[test]
    fn truncate_tiny_cap_never_exceeds() {
        // Cap smaller than the marker ("…(truncated)" = 14 bytes): the result
        // must still be <= max_bytes, so the marker is dropped rather than
        // overflowing the cap.
        let s = "x".repeat(1000);
        for cap in [1usize, 5, 13, 14, 15] {
            let out = truncate_attr(&s, cap);
            assert!(out.len() <= cap, "cap={cap} produced {} bytes", out.len());
        }
    }

    #[test]
    fn push_exception_event_full_set() {
        PROFILING_CONTEXT.with(|s| {
            s.borrow_mut().reset(
                crate::profiling::ProfilingMode::ApmOnly,
                "trace-x".into(),
                "root-x".into(),
            )
        });
        let id = PROFILING_CONTEXT.with(|s| s.borrow_mut().push(Arc::from("f"), vec![]));

        PROFILING_CONTEXT.with(|s| {
            let mut stack = s.borrow_mut();
            let span = stack.get_mut(id).unwrap();
            push_exception_event(
                span,
                "RuntimeException",
                Some("Payment declined"),
                Some("#0 /app/Pay.php(42): charge()\n#1 {main}"),
                4096,
                8192,
            );
            let ev = &span.events[0];
            assert_eq!(ev.name, "exception");
            assert_eq!(ev.attributes.len(), 3);
            assert_eq!(
                ev.attributes[0],
                (Arc::from("exception.type"), Arc::from("RuntimeException"))
            );
            assert_eq!(ev.attributes[1].0.as_ref(), "exception.message");
            assert_eq!(ev.attributes[1].1.as_ref(), "Payment declined");
            assert_eq!(ev.attributes[2].0.as_ref(), "exception.stacktrace");
        });
    }

    #[test]
    fn push_exception_event_skips_empty_optionals() {
        PROFILING_CONTEXT.with(|s| {
            s.borrow_mut().reset(
                crate::profiling::ProfilingMode::ApmOnly,
                "trace-y".into(),
                "root-y".into(),
            )
        });
        let id = PROFILING_CONTEXT.with(|s| s.borrow_mut().push(Arc::from("g"), vec![]));
        PROFILING_CONTEXT.with(|s| {
            let mut stack = s.borrow_mut();
            let span = stack.get_mut(id).unwrap();
            push_exception_event(span, "LogicException", None, Some(""), 4096, 8192);
            assert_eq!(span.events[0].attributes.len(), 1); // only exception.type
        });
    }

    #[test]
    fn push_exception_event_message_only_when_type_empty() {
        // A bare string reason (oxphp_apm_error('...')) has no class: the event
        // carries exception.message with no exception.type.
        PROFILING_CONTEXT.with(|s| {
            s.borrow_mut().reset(
                crate::profiling::ProfilingMode::ApmOnly,
                "trace-w".into(),
                "root-w".into(),
            )
        });
        let id = PROFILING_CONTEXT.with(|s| s.borrow_mut().push(Arc::from("i"), vec![]));
        PROFILING_CONTEXT.with(|s| {
            let mut stack = s.borrow_mut();
            let span = stack.get_mut(id).unwrap();
            push_exception_event(span, "", Some("gateway timeout"), None, 4096, 8192);
            let ev = &span.events[0];
            assert_eq!(ev.attributes.len(), 1);
            assert_eq!(ev.attributes[0].0.as_ref(), "exception.message");
            assert_eq!(ev.attributes[0].1.as_ref(), "gateway timeout");
        });
    }

    #[test]
    fn push_exception_event_truncates_message() {
        // Caps are passed in, so the test never mutates the process-wide globals
        // (which parallel tests read).
        PROFILING_CONTEXT.with(|s| {
            s.borrow_mut().reset(
                crate::profiling::ProfilingMode::ApmOnly,
                "trace-z".into(),
                "root-z".into(),
            )
        });
        let id = PROFILING_CONTEXT.with(|s| s.borrow_mut().push(Arc::from("h"), vec![]));
        let big_message = "SQL error: ".to_string() + &"A".repeat(10_000);
        PROFILING_CONTEXT.with(|s| {
            let mut stack = s.borrow_mut();
            let span = stack.get_mut(id).unwrap();
            push_exception_event(span, "PDOException", Some(&big_message), None, 128, 8192);
            let msg = &span.events[0].attributes[1];
            assert_eq!(msg.0.as_ref(), "exception.message");
            assert!(
                msg.1.as_ref().len() <= 128,
                "len was {}",
                msg.1.as_ref().len()
            );
            assert!(msg.1.as_ref().ends_with("…(truncated)"));
        });
    }
}
