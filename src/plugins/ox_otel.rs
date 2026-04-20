use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::trace::{
    SpanBuilder, SpanId, SpanKind, Status, TraceFlags, TraceId, TracerProvider as _,
};
use opentelemetry::KeyValue;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig, WithTonicConfig};
use opentelemetry_sdk::trace::{BatchConfigBuilder, Sampler, TracerProvider};
use opentelemetry_sdk::Resource;
use opentelemetry_semantic_conventions::trace as semconv;

use crate::events::Priority;
use crate::plugin::handler::{
    PluginCompleteHandler, PluginCompleteView, PluginRequestActions, PluginRequestHandler,
    PluginRequestView,
};
use crate::plugin::{Plugin, PluginContext, PluginError, PluginHealth};

/// OpenTelemetry plugin — exports HTTP server spans via OTLP.
///
/// Feature-gated behind `plugin-otel`. Reads standard `OTEL_*` env vars
/// for configuration. When enabled, signals `trace_context=true` via
/// `PluginContext::set_core_flag` so the built-in trace context handler
/// generates trace/span IDs.
pub struct OtelPlugin {
    enabled: bool,
    provider: Arc<OnceLock<TracerProvider>>,
    server_address: String,
}

impl Default for OtelPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl OtelPlugin {
    pub fn new() -> Self {
        Self {
            enabled: false,
            provider: Arc::new(OnceLock::new()),
            server_address: String::new(),
        }
    }

    /// Build the sampler from `OTEL_TRACES_SAMPLER` / `OTEL_TRACES_SAMPLER_ARG`.
    fn build_sampler() -> Sampler {
        let sampler_name = std::env::var("OTEL_TRACES_SAMPLER")
            .unwrap_or_else(|_| "parentbased_traceidratio".to_string());
        let sampler_arg: f64 = std::env::var("OTEL_TRACES_SAMPLER_ARG")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(1.0);

        match sampler_name.as_str() {
            "always_on" => Sampler::AlwaysOn,
            "always_off" => Sampler::AlwaysOff,
            "traceidratio" => Sampler::TraceIdRatioBased(sampler_arg),
            // parentbased_traceidratio (default) and parentbased_always_on/off
            "parentbased_always_on" => Sampler::ParentBased(Box::new(Sampler::AlwaysOn)),
            "parentbased_always_off" => Sampler::ParentBased(Box::new(Sampler::AlwaysOff)),
            _ => Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(sampler_arg))),
        }
    }

    /// Build the OTel `Resource` from env vars.
    fn build_resource() -> Resource {
        let service_name = std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "oxphp".into());
        let mut kvs = vec![KeyValue::new("service.name", service_name)];

        if let Ok(version) = std::env::var("OTEL_SERVICE_VERSION") {
            kvs.push(KeyValue::new("service.version", version));
        }

        // Parse OTEL_RESOURCE_ATTRIBUTES (key=value,key=value)
        if let Ok(attrs) = std::env::var("OTEL_RESOURCE_ATTRIBUTES") {
            for pair in attrs.split(',') {
                let pair = pair.trim();
                if let Some((k, v)) = pair.split_once('=') {
                    let k = k.trim().to_string();
                    let v = v.trim().to_string();
                    if !k.is_empty() {
                        kvs.push(KeyValue::new(k, v));
                    }
                }
            }
        }

        Resource::new(kvs)
    }

    /// Parse `OTEL_EXPORTER_OTLP_HEADERS` into key=value pairs.
    fn parse_headers() -> HashMap<String, String> {
        let mut headers = HashMap::new();
        if let Ok(raw) = std::env::var("OTEL_EXPORTER_OTLP_HEADERS") {
            for pair in raw.split(',') {
                let pair = pair.trim();
                if let Some((k, v)) = pair.split_once('=') {
                    let k = k.trim().to_string();
                    let v = v.trim().to_string();
                    if !k.is_empty() {
                        headers.insert(k, v);
                    }
                }
            }
        }
        headers
    }

    /// Initialize the TracerProvider with OTLP exporter.
    fn init_provider(&self) -> Result<TracerProvider, PluginError> {
        let protocol =
            std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_else(|_| "grpc".into());

        let timeout_ms: u64 = std::env::var("OTEL_EXPORTER_OTLP_TIMEOUT")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(10_000);
        let timeout = Duration::from_millis(timeout_ms);

        let headers = Self::parse_headers();

        let resource = Self::build_resource();
        let sampler = Self::build_sampler();

        let exporter = match protocol.as_str() {
            "http/protobuf" => {
                let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                    .unwrap_or_else(|_| "http://localhost:4318".into());

                let mut builder = opentelemetry_otlp::SpanExporter::builder()
                    .with_http()
                    .with_endpoint(endpoint)
                    .with_timeout(timeout);

                if !headers.is_empty() {
                    builder = builder.with_headers(headers);
                }

                builder
                    .build()
                    .map_err(|e| PluginError::Config(format!("OTLP HTTP exporter: {e}")))?
            }
            _ => {
                // Default: gRPC via tonic
                let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                    .unwrap_or_else(|_| "http://localhost:4317".into());

                let mut builder = opentelemetry_otlp::SpanExporter::builder()
                    .with_tonic()
                    .with_endpoint(endpoint)
                    .with_timeout(timeout);

                if !headers.is_empty() {
                    let mut metadata = tonic::metadata::MetadataMap::new();
                    for (k, v) in &headers {
                        if let Ok(key) =
                            k.parse::<tonic::metadata::MetadataKey<tonic::metadata::Ascii>>()
                        {
                            if let Ok(val) = v.parse() {
                                metadata.insert(key, val);
                            }
                        }
                    }
                    builder = builder.with_metadata(metadata);
                }

                builder
                    .build()
                    .map_err(|e| PluginError::Config(format!("OTLP gRPC exporter: {e}")))?
            }
        };

        // BatchConfigBuilder::default() reads OTEL_BSP_* env vars automatically
        let batch_config = BatchConfigBuilder::default().build();

        let batch_processor = opentelemetry_sdk::trace::BatchSpanProcessor::builder(
            exporter,
            opentelemetry_sdk::runtime::Tokio,
        )
        .with_batch_config(batch_config)
        .build();

        let provider = TracerProvider::builder()
            .with_span_processor(batch_processor)
            .with_sampler(sampler)
            .with_resource(resource)
            .build();

        Ok(provider)
    }
}

impl Plugin for OtelPlugin {
    fn name(&self) -> &'static str {
        "otel"
    }

    fn version(&self) -> &'static str {
        "0.1.0"
    }

    fn init(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        self.enabled = ctx
            .config("ENABLED")
            .or_else(|| ctx.config("OTEL_ENABLED"))
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        if !self.enabled {
            tracing::info!(
                plugin = "otel",
                "OTel plugin disabled (OTEL_ENABLED != true)"
            );
            ctx.expose_config("enabled", false);
            return Ok(());
        }

        // Read server address from LISTEN_ADDR
        self.server_address = std::env::var("LISTEN_ADDR").unwrap_or_default();

        // Enable trace context generation in built-in handler
        ctx.set_core_flag("trace_context", "true");

        // TracerProvider initialization is deferred to on_ready() because
        // BatchSpanProcessor requires an active Tokio runtime, and init()
        // runs before the runtime starts. The OnceLock<TracerProvider> is
        // filled in on_ready(); handlers check provider.get() and no-op if None.

        let protocol =
            std::env::var("OTEL_EXPORTER_OTLP_PROTOCOL").unwrap_or_else(|_| "grpc".into());
        let default_endpoint = if protocol == "http/protobuf" {
            "http://localhost:4318"
        } else {
            "http://localhost:4317"
        };

        tracing::info!(
            plugin = "otel",
            endpoint = %std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                .unwrap_or_else(|_| default_endpoint.into()),
            protocol = %protocol,
            "OTel plugin initialized (provider deferred to on_ready)"
        );

        ctx.expose_config("enabled", true);
        ctx.expose_config(
            "endpoint",
            std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                .unwrap_or_else(|_| default_endpoint.into()),
        );
        ctx.expose_config(
            "service_name",
            std::env::var("OTEL_SERVICE_NAME").unwrap_or_else(|_| "oxphp".into()),
        );

        // Share TracerProvider with other plugins (e.g. APM)
        ctx.register_service("otel.provider", Box::new(self.provider.clone()));

        // Register handlers
        ctx.on_request(OtelRequestHandler);
        ctx.on_complete(OtelCompleteHandler {
            provider: self.provider.clone(),
            server_address: self.server_address.clone(),
        });

        Ok(())
    }

    fn on_ready(&self) {
        if !self.enabled {
            return;
        }
        // Now inside Tokio runtime — safe to create BatchSpanProcessor
        match self.init_provider() {
            Ok(provider) => {
                if self.provider.set(provider).is_err() {
                    tracing::warn!(plugin = "otel", "TracerProvider already initialized");
                } else {
                    tracing::info!(
                        plugin = "otel",
                        "TracerProvider started (OTLP export active)"
                    );
                }
            }
            Err(e) => {
                tracing::error!(plugin = "otel", error = %e, "Failed to initialize TracerProvider");
            }
        }
    }

    fn shutdown(&mut self) {
        if !self.enabled {
            return;
        }
        if let Some(provider) = self.provider.get() {
            // Force flush remaining spans
            for result in provider.force_flush() {
                if let Err(e) = result {
                    tracing::warn!(error = %e, "OTel flush error during shutdown");
                }
            }
            if let Err(e) = provider.shutdown() {
                tracing::warn!(error = %e, "OTel provider shutdown error");
            }
        }
        tracing::info!(plugin = "otel", "OTel plugin shutdown complete");
    }

    fn health(&self) -> PluginHealth {
        if !self.enabled {
            return PluginHealth::Ok;
        }
        match self.provider.get() {
            Some(_) => PluginHealth::Ok,
            None => PluginHealth::Degraded,
        }
    }
}

// ─── Request handler ────────────────────────────────────────

struct OtelRequestHandler;

impl PluginRequestHandler for OtelRequestHandler {
    fn handle(&self, view: &PluginRequestView, actions: &mut PluginRequestActions) {
        let trace_id = match view.metadata("trace_id") {
            Some(v) if !v.is_empty() => v,
            _ => return,
        };
        let span_id = match view.metadata("span_id") {
            Some(v) if !v.is_empty() => v,
            _ => return,
        };

        // Store absolute start time in metadata — no DashMap, no lock contention
        let start_us = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64;
        actions.set_metadata("otel.start_us", start_us.to_string());

        // Derive request ID from trace context: first 16 chars of trace_id + first 8 of span_id
        let tid_prefix = &trace_id[..trace_id.len().min(16)];
        let sid_prefix = &span_id[..span_id.len().min(8)];
        actions.set_request_id(format!("{tid_prefix}-{sid_prefix}"));
    }

    fn priority(&self) -> Priority {
        -80
    }
}

// ─── Complete handler ───────────────────────────────────────

struct OtelCompleteHandler {
    provider: Arc<OnceLock<TracerProvider>>,
    server_address: String,
}

impl PluginCompleteHandler for OtelCompleteHandler {
    fn handle(&self, view: &PluginCompleteView) {
        // Quick guard checks before collecting owned data
        let trace_id_str = match view.metadata("trace_id") {
            Some(v) if !v.is_empty() => v,
            _ => return,
        };
        let span_id_str = match view.metadata("span_id") {
            Some(v) if !v.is_empty() => v,
            _ => return,
        };
        let start_us: u64 = match view.metadata("otel.start_us").and_then(|v| v.parse().ok()) {
            Some(v) => v,
            None => return,
        };
        if self.provider.get().is_none() {
            return;
        }

        // Collect owned data for background export — nothing below blocks the response
        let provider = self.provider.clone();
        let trace_id_owned = trace_id_str.to_string();
        let span_id_owned = span_id_str.to_string();
        let parent_span_id_owned = view
            .metadata("parent_span_id")
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());
        let trace_flags = view
            .metadata("trace_flags")
            .and_then(|s| u8::from_str_radix(s, 16).ok())
            .map(TraceFlags::new)
            .unwrap_or(TraceFlags::SAMPLED);
        let method = view.method.to_string();
        let path = view.path.to_string();
        let status_code = view.status;
        let remote_addr = view.remote_addr.ip().to_string();
        let request_id = view.request_id.to_string();
        let server_address = self.server_address.clone();
        let request_body_size = view.request_body_size;
        let response_size = view.response_size;
        let queue_wait_us = view.queue_wait_us.map(|v| v as i64);
        let php_exec_us = view.php_exec_us.map(|v| v as i64);

        // Span building + OTel export happens off the hot path
        tokio::spawn(async move {
            let provider = provider.get().unwrap(); // safe: checked above
            let tracer = provider.tracer("oxphp");

            let trace_id = match TraceId::from_hex(&trace_id_owned) {
                Ok(id) => id,
                Err(_) => return,
            };
            let span_id = match SpanId::from_hex(&span_id_owned) {
                Ok(id) => id,
                Err(_) => return,
            };
            let parent_span_id = parent_span_id_owned
                .as_deref()
                .and_then(|s| SpanId::from_hex(s).ok());

            let start_time = std::time::UNIX_EPOCH + std::time::Duration::from_micros(start_us);
            let end_time = std::time::SystemTime::now();

            let mut attributes = vec![
                KeyValue::new(semconv::HTTP_REQUEST_METHOD, method.clone()),
                KeyValue::new(semconv::URL_PATH, path.clone()),
                KeyValue::new(semconv::HTTP_RESPONSE_STATUS_CODE, i64::from(status_code)),
                KeyValue::new(semconv::CLIENT_ADDRESS, remote_addr),
                KeyValue::new("oxphp.request_id", request_id),
                KeyValue::new(semconv::SERVER_ADDRESS, server_address),
            ];
            if request_body_size > 0 {
                attributes.push(KeyValue::new(
                    "http.request.body.size",
                    request_body_size as i64,
                ));
            }
            if response_size > 0 {
                attributes.push(KeyValue::new(
                    "http.response.body.size",
                    response_size as i64,
                ));
            }
            if let Some(us) = queue_wait_us {
                attributes.push(KeyValue::new("oxphp.queue_wait_us", us));
            }
            if let Some(us) = php_exec_us {
                attributes.push(KeyValue::new("oxphp.php_exec_us", us));
            }

            let status = if status_code >= 500 {
                Status::error(format!("HTTP {status_code}"))
            } else {
                Status::Ok
            };

            let span_name = format!("{method} {path}");
            let mut builder = SpanBuilder::from_name(span_name)
                .with_trace_id(trace_id)
                .with_span_id(span_id)
                .with_kind(SpanKind::Server)
                .with_start_time(start_time)
                .with_end_time(end_time)
                .with_attributes(attributes)
                .with_status(status);

            if let Some(parent_sid) = parent_span_id {
                use opentelemetry::trace::{SpanContext, TraceContextExt};
                let parent_ctx =
                    SpanContext::new(trace_id, parent_sid, trace_flags, true, Default::default());
                let parent_otel_ctx =
                    opentelemetry::Context::new().with_remote_span_context(parent_ctx);
                drop(builder.start_with_context(&tracer, &parent_otel_ctx));
            } else {
                if trace_flags.is_sampled() {
                    builder.sampling_result = Some(opentelemetry::trace::SamplingResult {
                        decision: opentelemetry::trace::SamplingDecision::RecordAndSample,
                        attributes: Vec::new(),
                        trace_state: Default::default(),
                    });
                }
                drop(builder.start(&tracer));
            }
        });
    }

    fn priority(&self) -> Priority {
        -80
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventDispatcher;
    use crate::plugin::context::PluginDecoratorDef;
    use crate::plugin::handler::{PluginInternalHandler, PluginMetricsCollector};
    use crate::plugin::php::PluginNativeFunctionDef;

    // Mutex to serialize tests that manipulate env vars
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn init_otel_plugin(plugin: &mut OtelPlugin) -> HashMap<String, serde_json::Value> {
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
            "otel".into(),
            "__oxp_otel_".into(),
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
    fn test_otel_plugin_disabled_by_default() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("OTEL_ENABLED");
        let mut plugin = OtelPlugin::new();
        let config = init_otel_plugin(&mut plugin);

        assert_eq!(plugin.name(), "otel");
        assert_eq!(plugin.version(), "0.1.0");
        assert!(!plugin.enabled);
        assert_eq!(config.get("enabled"), Some(&serde_json::json!(false)));
        assert_eq!(plugin.health(), PluginHealth::Ok);
    }

    #[test]
    fn test_otel_plugin_name_and_version() {
        let plugin = OtelPlugin::new();
        assert_eq!(plugin.name(), "otel");
        assert_eq!(plugin.version(), "0.1.0");
    }

    #[test]
    fn test_otel_request_handler_no_metadata() {
        let handler = OtelRequestHandler;
        let method = http::Method::GET;
        let uri: http::Uri = "/test".parse().unwrap();
        let addr: std::net::SocketAddr = "127.0.0.1:8080".parse().unwrap();
        let headers = http::HeaderMap::new();
        let cookies = crate::plugin::cookies::PluginCookies { cookies: vec![] };

        let view = PluginRequestView::new(&method, &uri, addr, "req1", &headers, cookies, &[]);
        let mut actions = PluginRequestActions::new();
        handler.handle(&view, &mut actions);

        // No trace metadata => no start_us metadata, no request_id override
        assert!(actions.metadata.is_empty());
        assert!(actions.request_id_override.is_none());
    }

    #[test]
    fn test_otel_request_handler_with_trace_metadata() {
        let handler = OtelRequestHandler;
        let method = http::Method::GET;
        let uri: http::Uri = "/api/users".parse().unwrap();
        let addr: std::net::SocketAddr = "10.0.0.1:9999".parse().unwrap();
        let headers = http::HeaderMap::new();
        let cookies = crate::plugin::cookies::PluginCookies { cookies: vec![] };
        let metadata = vec![
            (
                "trace_id".to_string(),
                "4bf92f3577b34da6a3ce929d0e0e4736".to_string(),
            ),
            ("span_id".to_string(), "00f067aa0ba902b7".to_string()),
        ];

        let view =
            PluginRequestView::new(&method, &uri, addr, "req1", &headers, cookies, &metadata);
        let mut actions = PluginRequestActions::new();
        handler.handle(&view, &mut actions);

        // Should have stored start_us in metadata
        let start_meta = actions.metadata.iter().find(|(k, _)| k == "otel.start_us");
        assert!(start_meta.is_some());
        assert!(start_meta.unwrap().1.parse::<u64>().is_ok());

        // Should have overridden request_id
        assert_eq!(
            actions.request_id_override,
            Some("4bf92f3577b34da6-00f067aa".to_string())
        );
    }

    #[test]
    fn test_otel_request_handler_priority() {
        let handler = OtelRequestHandler;
        assert_eq!(handler.priority(), -80);
    }

    #[test]
    fn test_otel_complete_handler_priority() {
        let handler = OtelCompleteHandler {
            provider: Arc::new(OnceLock::new()),
            server_address: String::new(),
        };
        assert_eq!(handler.priority(), -80);
    }

    #[test]
    fn test_otel_complete_handler_no_metadata() {
        let handler = OtelCompleteHandler {
            provider: Arc::new(OnceLock::new()),
            server_address: String::new(),
        };
        let view = PluginCompleteView::new(
            "req1",
            "GET",
            "/test",
            200,
            std::time::Duration::from_millis(10),
            "127.0.0.1:8080".parse().unwrap(),
            0,
            1024,
            &[],
            &[],
            None,
            None,
            None,
        );
        handler.handle(&view); // should not panic
    }

    #[test]
    fn test_otel_complete_handler_no_start_us() {
        let handler = OtelCompleteHandler {
            provider: Arc::new(OnceLock::new()),
            server_address: String::new(),
        };
        let metadata = vec![
            (
                "trace_id".to_string(),
                "4bf92f3577b34da6a3ce929d0e0e4736".to_string(),
            ),
            ("span_id".to_string(), "00f067aa0ba902b7".to_string()),
        ];
        let view = PluginCompleteView::new(
            "req1",
            "GET",
            "/test",
            200,
            std::time::Duration::from_millis(10),
            "127.0.0.1:8080".parse().unwrap(),
            0,
            1024,
            &metadata,
            &[],
            None,
            None,
            None,
        );
        handler.handle(&view); // no start_us, no provider => should not panic
    }

    #[test]
    fn test_otel_shutdown_disabled() {
        let mut plugin = OtelPlugin::new();
        plugin.shutdown(); // should not panic
    }

    #[test]
    fn test_otel_health_disabled() {
        let plugin = OtelPlugin::new();
        assert_eq!(plugin.health(), PluginHealth::Ok);
    }

    #[test]
    fn test_build_sampler_defaults() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("OTEL_TRACES_SAMPLER");
        std::env::remove_var("OTEL_TRACES_SAMPLER_ARG");
        let sampler = OtelPlugin::build_sampler();
        // Default is parentbased_traceidratio
        assert!(matches!(sampler, Sampler::ParentBased(_)));
    }

    #[test]
    fn test_build_sampler_always_on() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_TRACES_SAMPLER", "always_on");
        let sampler = OtelPlugin::build_sampler();
        assert!(matches!(sampler, Sampler::AlwaysOn));
        std::env::remove_var("OTEL_TRACES_SAMPLER");
    }

    #[test]
    fn test_build_sampler_always_off() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_TRACES_SAMPLER", "always_off");
        let sampler = OtelPlugin::build_sampler();
        assert!(matches!(sampler, Sampler::AlwaysOff));
        std::env::remove_var("OTEL_TRACES_SAMPLER");
    }

    #[test]
    fn test_build_sampler_ratio() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_TRACES_SAMPLER", "traceidratio");
        std::env::set_var("OTEL_TRACES_SAMPLER_ARG", "0.5");
        let sampler = OtelPlugin::build_sampler();
        assert!(matches!(sampler, Sampler::TraceIdRatioBased(r) if (r - 0.5).abs() < f64::EPSILON));
        std::env::remove_var("OTEL_TRACES_SAMPLER");
        std::env::remove_var("OTEL_TRACES_SAMPLER_ARG");
    }

    #[test]
    fn test_build_resource_defaults() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("OTEL_SERVICE_NAME");
        std::env::remove_var("OTEL_SERVICE_VERSION");
        std::env::remove_var("OTEL_RESOURCE_ATTRIBUTES");
        let resource = OtelPlugin::build_resource();
        let sn = resource.get(opentelemetry::Key::new("service.name"));
        assert_eq!(sn.map(|v| v.to_string()), Some("oxphp".to_string()));
    }

    #[test]
    fn test_build_resource_custom() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_SERVICE_NAME", "my-app");
        std::env::set_var("OTEL_SERVICE_VERSION", "2.0.0");
        std::env::set_var("OTEL_RESOURCE_ATTRIBUTES", "env=prod,region=us-east-1");
        let resource = OtelPlugin::build_resource();

        assert_eq!(
            resource
                .get(opentelemetry::Key::new("service.name"))
                .map(|v| v.to_string()),
            Some("my-app".to_string())
        );
        assert_eq!(
            resource
                .get(opentelemetry::Key::new("service.version"))
                .map(|v| v.to_string()),
            Some("2.0.0".to_string())
        );
        assert_eq!(
            resource
                .get(opentelemetry::Key::new("env"))
                .map(|v| v.to_string()),
            Some("prod".to_string())
        );
        assert_eq!(
            resource
                .get(opentelemetry::Key::new("region"))
                .map(|v| v.to_string()),
            Some("us-east-1".to_string())
        );

        std::env::remove_var("OTEL_SERVICE_NAME");
        std::env::remove_var("OTEL_SERVICE_VERSION");
        std::env::remove_var("OTEL_RESOURCE_ATTRIBUTES");
    }

    #[test]
    fn test_request_id_derivation() {
        // Verify the format: first 16 of trace_id + "-" + first 8 of span_id
        let trace_id = "4bf92f3577b34da6a3ce929d0e0e4736";
        let span_id = "00f067aa0ba902b7";
        let tid_prefix = &trace_id[..16];
        let sid_prefix = &span_id[..8];
        let derived = format!("{tid_prefix}-{sid_prefix}");
        assert_eq!(derived, "4bf92f3577b34da6-00f067aa");
    }

    #[test]
    fn test_parse_headers_empty() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("OTEL_EXPORTER_OTLP_HEADERS");
        let headers = OtelPlugin::parse_headers();
        assert!(headers.is_empty());
    }

    #[test]
    fn test_parse_headers_single() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var(
            "OTEL_EXPORTER_OTLP_HEADERS",
            "Authorization=Bearer token123",
        );
        let headers = OtelPlugin::parse_headers();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer token123");
        std::env::remove_var("OTEL_EXPORTER_OTLP_HEADERS");
    }

    #[test]
    fn test_parse_headers_multiple() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var(
            "OTEL_EXPORTER_OTLP_HEADERS",
            "Authorization=Bearer tok,X-Custom=value42",
        );
        let headers = OtelPlugin::parse_headers();
        assert_eq!(headers.len(), 2);
        assert_eq!(headers.get("Authorization").unwrap(), "Bearer tok");
        assert_eq!(headers.get("X-Custom").unwrap(), "value42");
        std::env::remove_var("OTEL_EXPORTER_OTLP_HEADERS");
    }

    #[test]
    fn test_parse_headers_whitespace_trimming() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_EXPORTER_OTLP_HEADERS", " key = val , k2 = v2 ");
        let headers = OtelPlugin::parse_headers();
        assert_eq!(headers.len(), 2);
        assert_eq!(headers.get("key").unwrap(), "val");
        assert_eq!(headers.get("k2").unwrap(), "v2");
        std::env::remove_var("OTEL_EXPORTER_OTLP_HEADERS");
    }

    #[test]
    fn test_parse_headers_empty_key_skipped() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_EXPORTER_OTLP_HEADERS", "=bad,good=val");
        let headers = OtelPlugin::parse_headers();
        assert_eq!(headers.len(), 1);
        assert_eq!(headers.get("good").unwrap(), "val");
        std::env::remove_var("OTEL_EXPORTER_OTLP_HEADERS");
    }

    #[test]
    fn test_server_address_stored() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("OTEL_ENABLED");
        std::env::set_var("LISTEN_ADDR", "0.0.0.0:8080");
        let mut plugin = OtelPlugin::new();
        // Plugin is disabled so init won't try to connect to OTLP
        init_otel_plugin(&mut plugin);
        // server_address is only set when enabled, so check new() default
        assert_eq!(plugin.server_address, "");
        std::env::remove_var("LISTEN_ADDR");
    }

    #[test]
    fn test_complete_handler_has_server_address() {
        let handler = OtelCompleteHandler {
            provider: Arc::new(OnceLock::new()),
            server_address: "0.0.0.0:8080".to_string(),
        };
        assert_eq!(handler.server_address, "0.0.0.0:8080");
    }
}
