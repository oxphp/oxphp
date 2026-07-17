use std::collections::HashMap;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::trace::{
    SpanBuilder, SpanId, SpanKind, Status, TraceFlags, TraceId, TracerProvider as _,
};
use opentelemetry::KeyValue;
use opentelemetry_otlp::{WithExportConfig, WithHttpConfig, WithTonicConfig};
use opentelemetry_sdk::trace::{Sampler, SdkTracerProvider};
use opentelemetry_sdk::Resource;
use opentelemetry_semantic_conventions::trace as semconv;
use tonic::transport::ClientTlsConfig;

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
    provider: Arc<OnceLock<SdkTracerProvider>>,
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

    /// Parses `OTEL_TRACES_SAMPLER_ARG` with OTEL-spec-compliant validation.
    /// Invalid input is logged via `tracing::warn!` and falls back to safe defaults.
    fn parse_sampler_arg() -> f64 {
        let raw = match std::env::var("OTEL_TRACES_SAMPLER_ARG") {
            Ok(v) => v,
            Err(_) => return 1.0,
        };

        let parsed: f64 = match raw.parse() {
            Ok(v) => v,
            Err(_) => {
                tracing::warn!(
                    plugin = "otel",
                    value = %raw,
                    "invalid OTEL_TRACES_SAMPLER_ARG (parse error); defaulting to 1.0"
                );
                return 1.0;
            }
        };

        if parsed.is_nan() {
            tracing::warn!(
                plugin = "otel",
                "OTEL_TRACES_SAMPLER_ARG is NaN; defaulting to 1.0"
            );
            return 1.0;
        }
        if parsed < 0.0 {
            tracing::warn!(
                plugin = "otel",
                value = parsed,
                "OTEL_TRACES_SAMPLER_ARG below 0.0; clamped to 0.0"
            );
            return 0.0;
        }
        if parsed > 1.0 {
            tracing::warn!(
                plugin = "otel",
                value = parsed,
                "OTEL_TRACES_SAMPLER_ARG above 1.0; clamped to 1.0"
            );
            return 1.0;
        }
        parsed
    }

    /// Build the sampler from `OTEL_TRACES_SAMPLER` / `OTEL_TRACES_SAMPLER_ARG`.
    fn build_sampler() -> Sampler {
        let sampler_arg = Self::parse_sampler_arg();
        let raw_name = std::env::var("OTEL_TRACES_SAMPLER")
            .unwrap_or_else(|_| "parentbased_traceidratio".to_string());

        match raw_name.as_str() {
            "always_on" => Sampler::AlwaysOn,
            "always_off" => Sampler::AlwaysOff,
            "traceidratio" => Sampler::TraceIdRatioBased(sampler_arg),
            "parentbased_always_on" => Sampler::ParentBased(Box::new(Sampler::AlwaysOn)),
            "parentbased_always_off" => Sampler::ParentBased(Box::new(Sampler::AlwaysOff)),
            "parentbased_traceidratio" => {
                Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(sampler_arg)))
            }
            other => {
                tracing::warn!(
                    plugin = "otel",
                    value = %other,
                    "unknown OTEL_TRACES_SAMPLER; falling back to parentbased_traceidratio"
                );
                Sampler::ParentBased(Box::new(Sampler::TraceIdRatioBased(sampler_arg)))
            }
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

        Resource::builder().with_attributes(kvs).build()
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

    /// Whether an OTLP endpoint URL uses TLS (an `https://` scheme, matched
    /// case-insensitively). Gates attaching a tonic `ClientTlsConfig`: the
    /// connector — and its eager system-trust-store load — is only wanted for
    /// TLS endpoints, never for plaintext `http://` or scheme-less ones.
    fn endpoint_uses_tls(endpoint: &str) -> bool {
        endpoint
            .split_once("://")
            .is_some_and(|(scheme, _)| scheme.eq_ignore_ascii_case("https"))
    }

    /// Initialize the SdkTracerProvider with OTLP exporter.
    fn init_provider(&self) -> Result<SdkTracerProvider, PluginError> {
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
                // trim(): strips a stray newline/space from templated env values
                // (e.g. Helm), matching how headers are parsed above.
                let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                    .map(|s| s.trim().to_string())
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
                // trim(): strips a stray newline/space from templated env values
                // (e.g. Helm) so the scheme check and the URI both see a clean value.
                let endpoint = std::env::var("OTEL_EXPORTER_OTLP_ENDPOINT")
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| "http://localhost:4317".into());
                let use_tls = Self::endpoint_uses_tls(&endpoint);

                let mut builder = opentelemetry_otlp::SpanExporter::builder()
                    .with_tonic()
                    .with_endpoint(endpoint)
                    .with_timeout(timeout);

                // Attach TLS only for https endpoints. tonic builds the TLS
                // connector eagerly inside build(), and with_native_roots() loads
                // the system trust store there — so attaching it to a plaintext
                // http:// endpoint would make build() fail with NativeCertsNotFound
                // on a host without a CA store (e.g. a minimal image lacking
                // ca-certificates), silently disabling all trace export. Gating on
                // the scheme keeps plaintext working everywhere. Native roots =
                // the system trust store (rustls-native-certs).
                if use_tls {
                    builder = builder.with_tls_config(ClientTlsConfig::new().with_native_roots());
                }

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

        let provider = SdkTracerProvider::builder()
            .with_batch_exporter(exporter)
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
        let raw = ctx.config("ENABLED").or_else(|| ctx.config("OTEL_ENABLED"));
        self.enabled = crate::config::parse_bool_opt("OTEL_ENABLED", raw.as_deref(), false)
            .map_err(|e| PluginError::Config(e.to_string()))?;

        if !self.enabled {
            tracing::info!(
                plugin = "otel",
                "OTel plugin disabled (OTEL_ENABLED is falsy or unset)"
            );
            ctx.expose_config("enabled", false);
            return Ok(());
        }

        // Read server address from LISTEN_ADDR
        self.server_address = std::env::var("LISTEN_ADDR").unwrap_or_default();

        // Enable trace context generation in built-in handler
        ctx.set_core_flag("trace_context", "true");

        // SdkTracerProvider initialization is deferred to on_ready() because
        // BatchSpanProcessor requires an active Tokio runtime, and init()
        // runs before the runtime starts. The OnceLock<SdkTracerProvider> is
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

        // Share SdkTracerProvider with other plugins (e.g. APM)
        ctx.register_service("otel.provider", Box::new(self.provider.clone()));

        // Byte caps for the auto-captured root-span exception event. Read the
        // same bare `OTEL_APM_*_MAX_BYTES` knobs as the APM child-span path so a
        // single operator setting drives both, with the same warn-on-typo and
        // blank-is-default behavior. `0` = no truncation.
        let message_max = read_byte_cap(ctx, "OTEL_APM_MESSAGE_MAX_BYTES", 4096);
        let stacktrace_max = read_byte_cap(ctx, "OTEL_APM_STACKTRACE_MAX_BYTES", 8192);

        // Register handlers
        ctx.on_request(OtelRequestHandler);
        ctx.on_complete(OtelCompleteHandler {
            provider: self.provider.clone(),
            server_address: self.server_address.clone(),
            message_max,
            stacktrace_max,
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
                    tracing::warn!(plugin = "otel", "SdkTracerProvider already initialized");
                } else {
                    tracing::info!(
                        plugin = "otel",
                        "SdkTracerProvider started (OTLP export active)"
                    );
                }
            }
            Err(e) => {
                tracing::error!(plugin = "otel", error = %e, "Failed to initialize SdkTracerProvider");
            }
        }
    }

    fn shutdown(&mut self) {
        if !self.enabled {
            return;
        }
        if let Some(provider) = self.provider.get() {
            // Force flush remaining spans
            if let Err(e) = provider.force_flush() {
                tracing::warn!(error = %e, "OTel flush error during shutdown");
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

// ─── Unhandled-exception event ──────────────────────────────

/// Parse a byte-cap env var through `PluginContext::config` (so the plugin reads
/// its own configuration the same way every other plugin does, honoring a
/// plugin-prefixed override), warning — rather than silently defaulting — on a
/// malformed value so an operator's typo surfaces. Unset or blank uses
/// `default`. Mirrors the APM plugin's `read_cap`, so the shared
/// `OTEL_APM_*_MAX_BYTES` knobs behave the same on the root-span path (here) and
/// the child-span path.
fn read_byte_cap(ctx: &PluginContext, env: &str, default: usize) -> usize {
    match ctx.config(env).as_deref().map(str::trim) {
        None | Some("") => default,
        Some(v) => match v.parse() {
            Ok(n) => n,
            Err(_) => {
                tracing::warn!(
                    plugin = "otel",
                    env,
                    value = %v,
                    default,
                    "invalid byte cap; expected a non-negative integer, using default"
                );
                default
            }
        },
    }
}

/// Truncate a string to at most `max_bytes` on a UTF-8 boundary, keeping the head
/// (so a stacktrace's root frame `#0` survives) and appending a `…(truncated)`
/// marker when the cap leaves room for it. `0` disables truncation. This is the
/// single copy shared by both exception-event paths: the APM plugin (which
/// depends on this one) reuses it for child-span events so root-span and
/// child-span exception attributes truncate identically.
pub(crate) fn truncate_attr(s: &str, max_bytes: usize) -> std::borrow::Cow<'_, str> {
    if max_bytes == 0 || s.len() <= max_bytes {
        return std::borrow::Cow::Borrowed(s);
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
    std::borrow::Cow::Owned(out)
}

/// Build the OTel `exception` event for an unhandled exception / fatal error.
/// `exception.file`/`exception.line` are an OxPHP extension (OTel standardizes
/// only `exception.{type,message,stacktrace,escaped}`).
fn exception_event(
    exc: &crate::types::CapturedException,
    timestamp: std::time::SystemTime,
    message_max: usize,
    stacktrace_max: usize,
) -> opentelemetry::trace::Event {
    // Strip any embedded NUL from the class name — an anonymous class arrives as
    // "<parent>@anonymous\0<file>:<line>$<hash>" (worker capture is length-
    // delimited), and a NUL would truncate the type again downstream.
    let ty = if exc.exception_type.contains('\0') {
        exc.exception_type.replace('\0', "")
    } else {
        exc.exception_type.clone()
    };
    // Omit `exception.type` when empty (a degenerate parse), matching the APM
    // child-span path — some backends drop an event whose type is a blank string.
    let mut attrs = Vec::with_capacity(6);
    if !ty.is_empty() {
        attrs.push(KeyValue::new("exception.type", ty));
    }
    if let Some(m) = &exc.message {
        attrs.push(KeyValue::new(
            "exception.message",
            truncate_attr(m, message_max).into_owned(),
        ));
    }
    if let Some(t) = &exc.stacktrace {
        attrs.push(KeyValue::new(
            "exception.stacktrace",
            truncate_attr(t, stacktrace_max).into_owned(),
        ));
    }
    if let Some(f) = &exc.file {
        attrs.push(KeyValue::new("exception.file", f.clone()));
    }
    if let Some(l) = exc.line {
        attrs.push(KeyValue::new("exception.line", l as i64));
    }
    attrs.push(KeyValue::new("oxphp.event.kind", "exception"));
    opentelemetry::trace::Event::new("exception", timestamp, attrs, 0)
}

// ─── Complete handler ───────────────────────────────────────

struct OtelCompleteHandler {
    provider: Arc<OnceLock<SdkTracerProvider>>,
    server_address: String,
    /// Byte caps for the auto-captured `exception` event's message/stacktrace.
    /// `0` disables truncation. Shared knob names with the APM plugin.
    message_max: usize,
    stacktrace_max: usize,
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

        // Auto-capture the unhandled exception / fatal that failed a 5xx request,
        // so the root SERVER span carries it without any PHP-side integration.
        // Build the (already byte-capped) event here, BEFORE the spawn, so a
        // multi-megabyte message/stacktrace is truncated on the hot path and only
        // the bounded event — not the full-size `CapturedException` — is moved
        // into the background task.
        let unhandled_event = if status_code >= 500 {
            crate::php::unhandled_exception::extract_unhandled_exception(view.php_errors).map(
                |exc| {
                    exception_event(
                        &exc,
                        std::time::SystemTime::now(),
                        self.message_max,
                        self.stacktrace_max,
                    )
                },
            )
        } else {
            None
        };

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

            if let Some(event) = unhandled_event {
                builder = builder.with_events(vec![event]);
            }

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
    use std::borrow::Cow;

    // `truncate_attr` is owned by this plugin (the APM plugin re-uses it), so its
    // unit tests live here — an otel-only build still exercises the helper.
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
    fn exception_event_carries_all_attrs() {
        use crate::types::CapturedException;
        let exc = CapturedException {
            exception_type: "RuntimeException".into(),
            message: Some("boom".into()),
            stacktrace: Some("#0 {main}".into()),
            file: Some("/app/x.php".into()),
            line: Some(9),
        };
        let ev = super::exception_event(&exc, std::time::UNIX_EPOCH, 4096, 8192);
        assert_eq!(ev.name, "exception");
        let get = |k: &str| {
            ev.attributes
                .iter()
                .find(|kv| kv.key.as_str() == k)
                .map(|kv| kv.value.to_string())
        };
        assert_eq!(get("exception.type").as_deref(), Some("RuntimeException"));
        assert_eq!(get("exception.message").as_deref(), Some("boom"));
        assert_eq!(get("exception.stacktrace").as_deref(), Some("#0 {main}"));
        assert_eq!(get("exception.file").as_deref(), Some("/app/x.php"));
        assert_eq!(get("exception.line").as_deref(), Some("9"));
        assert_eq!(get("oxphp.event.kind").as_deref(), Some("exception"));
    }

    #[test]
    fn exception_event_truncates_and_skips_missing() {
        use crate::types::CapturedException;
        let exc = CapturedException {
            exception_type: "E_ERROR".into(),
            message: Some("abcdef".into()),
            stacktrace: None,
            file: None,
            line: None,
        };
        let ev = super::exception_event(&exc, std::time::UNIX_EPOCH, 3, 8192);
        let get = |k: &str| {
            ev.attributes
                .iter()
                .find(|kv| kv.key.as_str() == k)
                .map(|kv| kv.value.to_string())
        };
        assert_eq!(get("exception.message").as_deref(), Some("abc")); // truncated to 3 bytes
        assert!(get("exception.stacktrace").is_none()); // skipped
        assert!(get("exception.file").is_none());
        assert!(get("exception.line").is_none());
    }

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
            message_max: 4096,
            stacktrace_max: 8192,
        };
        assert_eq!(handler.priority(), -80);
    }

    #[test]
    fn test_otel_complete_handler_no_metadata() {
        let handler = OtelCompleteHandler {
            provider: Arc::new(OnceLock::new()),
            server_address: String::new(),
            message_max: 4096,
            stacktrace_max: 8192,
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
            message_max: 4096,
            stacktrace_max: 8192,
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
    fn test_build_sampler_arg_parse_error_falls_back_to_one() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_TRACES_SAMPLER", "traceidratio");
        std::env::set_var("OTEL_TRACES_SAMPLER_ARG", "not-a-number");
        let sampler = OtelPlugin::build_sampler();
        assert!(matches!(sampler, Sampler::TraceIdRatioBased(r) if (r - 1.0).abs() < f64::EPSILON));
        std::env::remove_var("OTEL_TRACES_SAMPLER");
        std::env::remove_var("OTEL_TRACES_SAMPLER_ARG");
    }

    #[test]
    fn test_build_sampler_arg_above_one_is_clamped() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_TRACES_SAMPLER", "traceidratio");
        std::env::set_var("OTEL_TRACES_SAMPLER_ARG", "2.5");
        let sampler = OtelPlugin::build_sampler();
        assert!(matches!(sampler, Sampler::TraceIdRatioBased(r) if (r - 1.0).abs() < f64::EPSILON));
        std::env::remove_var("OTEL_TRACES_SAMPLER");
        std::env::remove_var("OTEL_TRACES_SAMPLER_ARG");
    }

    #[test]
    fn test_build_sampler_arg_below_zero_is_clamped() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_TRACES_SAMPLER", "traceidratio");
        std::env::set_var("OTEL_TRACES_SAMPLER_ARG", "-0.5");
        let sampler = OtelPlugin::build_sampler();
        assert!(matches!(sampler, Sampler::TraceIdRatioBased(r) if r.abs() < f64::EPSILON));
        std::env::remove_var("OTEL_TRACES_SAMPLER");
        std::env::remove_var("OTEL_TRACES_SAMPLER_ARG");
    }

    #[test]
    fn test_build_sampler_arg_nan_falls_back_to_one() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_TRACES_SAMPLER", "traceidratio");
        std::env::set_var("OTEL_TRACES_SAMPLER_ARG", "NaN");
        let sampler = OtelPlugin::build_sampler();
        assert!(matches!(sampler, Sampler::TraceIdRatioBased(r) if (r - 1.0).abs() < f64::EPSILON));
        std::env::remove_var("OTEL_TRACES_SAMPLER");
        std::env::remove_var("OTEL_TRACES_SAMPLER_ARG");
    }

    #[test]
    fn test_build_sampler_arg_zero_passes_through() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_TRACES_SAMPLER", "traceidratio");
        std::env::set_var("OTEL_TRACES_SAMPLER_ARG", "0.0");
        let sampler = OtelPlugin::build_sampler();
        assert!(matches!(sampler, Sampler::TraceIdRatioBased(r) if r.abs() < f64::EPSILON));
        std::env::remove_var("OTEL_TRACES_SAMPLER");
        std::env::remove_var("OTEL_TRACES_SAMPLER_ARG");
    }

    #[test]
    fn test_build_sampler_arg_one_passes_through() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_TRACES_SAMPLER", "traceidratio");
        std::env::set_var("OTEL_TRACES_SAMPLER_ARG", "1.0");
        let sampler = OtelPlugin::build_sampler();
        assert!(matches!(sampler, Sampler::TraceIdRatioBased(r) if (r - 1.0).abs() < f64::EPSILON));
        std::env::remove_var("OTEL_TRACES_SAMPLER");
        std::env::remove_var("OTEL_TRACES_SAMPLER_ARG");
    }

    #[test]
    fn test_build_sampler_unknown_name_falls_back() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("OTEL_TRACES_SAMPLER", "garbage_value_xyz");
        std::env::remove_var("OTEL_TRACES_SAMPLER_ARG");
        let sampler = OtelPlugin::build_sampler();
        // Unknown name falls back to parentbased_traceidratio. The inner Sampler
        // is wrapped in Box<dyn ShouldSample> and cannot be downcast, so we only
        // assert on the outer variant — the inner arg (1.0 by default) is
        // exercised by the other tests in this module.
        assert!(matches!(sampler, Sampler::ParentBased(_)));
        std::env::remove_var("OTEL_TRACES_SAMPLER");
    }

    #[test]
    fn test_build_resource_defaults() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::remove_var("OTEL_SERVICE_NAME");
        std::env::remove_var("OTEL_SERVICE_VERSION");
        std::env::remove_var("OTEL_RESOURCE_ATTRIBUTES");
        let resource = OtelPlugin::build_resource();
        let sn = resource.get(&opentelemetry::Key::new("service.name"));
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
                .get(&opentelemetry::Key::new("service.name"))
                .map(|v| v.to_string()),
            Some("my-app".to_string())
        );
        assert_eq!(
            resource
                .get(&opentelemetry::Key::new("service.version"))
                .map(|v| v.to_string()),
            Some("2.0.0".to_string())
        );
        assert_eq!(
            resource
                .get(&opentelemetry::Key::new("env"))
                .map(|v| v.to_string()),
            Some("prod".to_string())
        );
        assert_eq!(
            resource
                .get(&opentelemetry::Key::new("region"))
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
            message_max: 4096,
            stacktrace_max: 8192,
        };
        assert_eq!(handler.server_address, "0.0.0.0:8080");
    }

    #[test]
    fn test_endpoint_uses_tls_scheme_gate() {
        // Regression guard for the scheme gate: only https endpoints may pull in
        // the tonic TLS connector (whose native-roots load is eager and fails on
        // a CA-less host). http:// and scheme-less endpoints must stay plaintext.
        assert!(OtelPlugin::endpoint_uses_tls("https://collector:4317"));
        assert!(OtelPlugin::endpoint_uses_tls("HTTPS://collector:4317")); // scheme is case-insensitive
        assert!(!OtelPlugin::endpoint_uses_tls("http://localhost:4317"));
        assert!(!OtelPlugin::endpoint_uses_tls("grpc://collector:4317"));
        assert!(!OtelPlugin::endpoint_uses_tls("localhost:4317")); // no scheme
    }

    #[test]
    fn test_grpc_plaintext_exporter_builds() {
        // Regression guard for the scheme gate: a plaintext http:// gRPC endpoint
        // must build WITHOUT a ClientTlsConfig, so it succeeds even on a host with
        // no CA store. (The pre-fix code attached TLS unconditionally, which made
        // build() fail with NativeCertsNotFound on a CA-less host.) This path is
        // environment-independent — unlike an https:// build, whose eager
        // with_native_roots() load depends on the host trust store, so that path is
        // left to the live smoke and is not asserted here. The scheme *decision* is
        // covered by test_endpoint_uses_tls_scheme_gate above. build() is lazy
        // (connect_lazy), so no collector is contacted.

        // A Tokio runtime context is required: SdkTracerProvider::build() starts
        // a BatchSpanProcessor that spawns a background task.
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let _guard = rt.enter();

        let plugin = OtelPlugin::new();

        // Capture the result and clean up env inside the locked scope, then release
        // ENV_MUTEX BEFORE asserting: a failing assert must not poison the mutex
        // (which would cascade panics through every other test that locks it).
        let http = {
            let _lock = ENV_MUTEX.lock().unwrap();
            std::env::set_var("OTEL_EXPORTER_OTLP_PROTOCOL", "grpc");
            std::env::set_var("OTEL_EXPORTER_OTLP_ENDPOINT", "http://localhost:4317");
            let http = plugin.init_provider();
            std::env::remove_var("OTEL_EXPORTER_OTLP_PROTOCOL");
            std::env::remove_var("OTEL_EXPORTER_OTLP_ENDPOINT");
            http
        };

        assert!(
            http.is_ok(),
            "gRPC plaintext exporter should build without a CA store: {:?}",
            http.err()
        );
    }
}
