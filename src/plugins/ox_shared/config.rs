//! SHARED_* env var reader. Uses PluginContext::config_prefixed to
//! avoid collision with other plugins' bare keys.

use crate::config::parse_bool_opt;
use crate::plugin::{PluginContext, PluginError};

#[derive(Debug, Clone)]
pub struct SharedConfig {
    pub enabled: bool,
    pub max_entries: usize,
    pub max_bytes: u64,
    pub metrics_enabled: bool,
    pub introspection_enabled: bool,
    pub introspection_preview_enabled: bool,
    pub cycle_detect_depth: usize,
    pub cycle_detect_edges: usize,
    /// Per-value serialised size cap (bytes). A single Map/Channel value
    /// whose portbuf encoding exceeds this throws ValueTooLargeException.
    pub max_value_size: usize,
    /// Upper bound (bytes) on a single Channel's pre-allocated slot array.
    /// `capacity * slot_bytes` above this throws CapacityException at
    /// construction, converting an allocation-bomb abort into a catchable
    /// error. Env: SHARED_MAX_CHANNEL_BYTES (default 64 MiB). The effective
    /// budget is clamped up to at least one slot, so a zero or sub-slot
    /// value can never reject a minimal capacity-1 channel.
    pub max_channel_bytes: u64,
    pub lock_diagnostics: LockDiagnosticsLevel,
    pub lock_poll_interval_ms: u64,
    pub preview_string_limit: usize,
    pub preview_array_limit: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockDiagnosticsLevel {
    Off,
    Warn,
    Strict,
}

impl SharedConfig {
    pub fn from_ctx(ctx: &PluginContext) -> Result<Self, PluginError> {
        for (key, advice) in DEPRECATED_KEYS {
            if let Some(name) = deprecated_env_name(ctx, key) {
                tracing::warn!(
                    env_var = %name,
                    "{name} is deprecated and ignored; {advice}"
                );
            }
        }

        Ok(Self {
            enabled: shared_bool(ctx, "ENABLED", true)?,
            max_entries: parse_usize(shared_value(ctx, "MAX_ENTRIES"), 100_000),
            max_bytes: parse_u64(shared_value(ctx, "MAX_BYTES"), 1_073_741_824),
            metrics_enabled: shared_bool(ctx, "METRICS_ENABLED", true)?,
            introspection_enabled: shared_bool(ctx, "INTROSPECTION_ENABLED", true)?,
            introspection_preview_enabled: shared_bool(ctx, "INTROSPECTION_PREVIEW_ENABLED", true)?,
            cycle_detect_depth: parse_usize(shared_value(ctx, "CYCLE_DETECT_DEPTH"), 16),
            cycle_detect_edges: parse_usize(shared_value(ctx, "CYCLE_DETECT_EDGES"), 10_000),
            max_value_size: parse_usize(shared_value(ctx, "MAX_VALUE_SIZE"), 1 << 20),
            max_channel_bytes: parse_u64(shared_value(ctx, "MAX_CHANNEL_BYTES"), 64 << 20),
            lock_diagnostics: parse_lock_diag(shared_value(ctx, "LOCK_DIAGNOSTICS")),
            lock_poll_interval_ms: parse_u64(shared_value(ctx, "LOCK_POLL_INTERVAL_MS"), 100),
            preview_string_limit: parse_usize(shared_value(ctx, "PREVIEW_STRING_LIMIT"), 256),
            preview_array_limit: parse_usize(shared_value(ctx, "PREVIEW_ARRAY_LIMIT"), 20),
        })
    }
}

/// Settings still read, only to tell the operator they do nothing, with what
/// to reach for instead.
///
/// - `SHUTDOWN_TIMEOUT_SECONDS`: `SharedRegistry::drain` is synchronous, and
///   the overall shutdown deadline is owned by the connection-drain loop in
///   `main.rs`.
/// - `SOFT_LIMIT_RATIO`: nothing ever acted on the threshold; the saturation
///   gauge is where an alert on it belongs.
/// - `POISON_STRICT`: no primitive consulted it. An exception thrown inside a
///   `Mutex` closure never poisons the mutex, and whether a throwing `Once`
///   factory is terminal is chosen per instance by its constructor argument.
const DEPRECATED_KEYS: [(&str, &str); 3] = [
    (
        "SHUTDOWN_TIMEOUT_SECONDS",
        "graceful shutdown is bounded by DRAIN_TIMEOUT_SECONDS",
    ),
    (
        "SOFT_LIMIT_RATIO",
        "alert on oxphp_shared_capacity_saturation instead",
    ),
    (
        "POISON_STRICT",
        "pass Once\\FailureMode::Poison to the Once constructor instead",
    ),
];

/// The spelling under which a deprecated `key` is set, if it is.
///
/// Only the documented public form (`SHARED_*`) and the plugin-prefixed
/// fallback (`OX_SHARED_*`) count. The bare-key tier of `shared_env` is the
/// last-resort fallback for active settings and was never part of a
/// deprecation's documented surface, so an unrelated
/// `SHUTDOWN_TIMEOUT_SECONDS` set for some other piece of software is not
/// reported.
fn deprecated_env_name(ctx: &PluginContext, key: &str) -> Option<String> {
    let public = format!("SHARED_{key}");
    if std::env::var(&public).is_ok() {
        Some(public)
    } else if ctx.config_prefixed(key).is_some() {
        Some(format!("OX_SHARED_{key}"))
    } else {
        None
    }
}

/// Convenience wrapper: drop the matched-var-name tag for callers that don't
/// produce parse errors (numeric/enum parsers fall back to default silently).
fn shared_value(ctx: &PluginContext, key: &str) -> Option<String> {
    shared_env(ctx, key).map(|(_, v)| v)
}

/// Read a Shared-plugin boolean via [`shared_env`] then strict-parse it.
/// Errors are tagged with the *exact* env var name that supplied the value
/// (`SHARED_*`, `OX_SHARED_*`, or bare key) so the operator finds the right
/// variable instead of chasing a misleading prefix.
fn shared_bool(ctx: &PluginContext, key: &str, default: bool) -> Result<bool, PluginError> {
    let resolved = shared_env(ctx, key);
    let (var_name, val) = match resolved {
        Some((name, v)) => (name, Some(v)),
        None => (format!("SHARED_{key}"), None),
    };
    parse_bool_opt(&var_name, val.as_deref(), default)
        .map_err(|e| PluginError::Config(e.to_string()))
}

/// Read a Shared-plugin env var, trying three lookups in priority order:
///   1. `SHARED_{KEY}`          — documented public API
///   2. `OX_SHARED_{KEY}`       — plugin-prefixed fallback
///   3. `{KEY}`                 — bare key last resort
///
/// Returns the matched env var name alongside the value so callers can tag
/// downstream errors with the actual variable the operator set.
fn shared_env(ctx: &PluginContext, key: &str) -> Option<(String, String)> {
    let public = format!("SHARED_{key}");
    if let Ok(v) = std::env::var(&public) {
        return Some((public, v));
    }
    if let Some(v) = ctx.config_prefixed(key) {
        return Some((format!("OX_SHARED_{key}"), v));
    }
    std::env::var(key).ok().map(|v| (key.to_string(), v))
}

fn parse_usize(val: Option<String>, default: usize) -> usize {
    val.and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn parse_u64(val: Option<String>, default: u64) -> u64 {
    val.and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn parse_lock_diag(val: Option<String>) -> LockDiagnosticsLevel {
    let default = if cfg!(debug_assertions) {
        LockDiagnosticsLevel::Strict
    } else {
        LockDiagnosticsLevel::Warn
    };
    match val.as_deref() {
        Some("off") => LockDiagnosticsLevel::Off,
        Some("warn") => LockDiagnosticsLevel::Warn,
        Some("strict") => LockDiagnosticsLevel::Strict,
        _ => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lock_diag_defaults() {
        let got = parse_lock_diag(None);
        if cfg!(debug_assertions) {
            assert_eq!(got, LockDiagnosticsLevel::Strict);
        } else {
            assert_eq!(got, LockDiagnosticsLevel::Warn);
        }
    }

    #[test]
    fn parse_lock_diag_explicit() {
        assert_eq!(
            parse_lock_diag(Some("off".into())),
            LockDiagnosticsLevel::Off
        );
        assert_eq!(
            parse_lock_diag(Some("warn".into())),
            LockDiagnosticsLevel::Warn
        );
        assert_eq!(
            parse_lock_diag(Some("strict".into())),
            LockDiagnosticsLevel::Strict
        );
    }

    #[test]
    fn parse_usize_bad_input_uses_default() {
        assert_eq!(parse_usize(Some("not-a-number".into()), 42), 42);
    }

    // Mirrors PluginContext::new — keeping the wide arg list keeps the test
    // scaffolding obvious instead of hiding bookkeeping behind a builder.
    #[allow(clippy::too_many_arguments)]
    fn build_ctx<'a>(
        dispatcher: &'a mut crate::events::EventDispatcher,
        services: &'a mut std::collections::HashMap<String, Box<dyn std::any::Any + Send + Sync>>,
        config_values: &'a mut std::collections::HashMap<String, serde_json::Value>,
        metrics_collectors: &'a mut Vec<Box<dyn crate::plugin::handler::PluginMetricsCollector>>,
        internal_routes: &'a mut std::collections::HashMap<
            String,
            Box<dyn crate::plugin::handler::PluginInternalHandler>,
        >,
        internal_route_prefixes: &'a mut Vec<(
            String,
            Box<dyn crate::plugin::handler::PluginInternalHandler>,
        )>,
        native_php_functions: &'a mut Vec<crate::plugin::php::PluginNativeFunctionDef>,
        decorators: &'a mut Vec<crate::plugin::context::PluginDecoratorDef>,
        php_classes: &'a mut Vec<crate::plugin::builders::definitions::PhpClassDef>,
        php_interfaces: &'a mut Vec<crate::plugin::builders::definitions::PhpInterfaceDef>,
        php_enums: &'a mut Vec<crate::plugin::builders::definitions::PhpEnumDef>,
        php_attributes: &'a mut Vec<crate::plugin::builders::definitions::PhpAttributeDef>,
        php_functions: &'a mut Vec<crate::plugin::builders::definitions::PhpFunctionDef>,
        core_flags: &'a mut std::collections::HashMap<String, String>,
    ) -> PluginContext<'a> {
        PluginContext::new(
            "ox_shared".into(),
            "__oxp_shared_".into(),
            dispatcher,
            services,
            config_values,
            metrics_collectors,
            internal_routes,
            internal_route_prefixes,
            native_php_functions,
            decorators,
            php_classes,
            php_interfaces,
            php_enums,
            php_attributes,
            php_functions,
            core_flags,
        )
    }

    macro_rules! with_ctx {
        ($body:expr) => {{
            let mut dispatcher = crate::events::EventDispatcher::new();
            let mut services = std::collections::HashMap::new();
            let mut config_values = std::collections::HashMap::new();
            let mut metrics_collectors = Vec::new();
            let mut internal_routes = std::collections::HashMap::new();
            let mut internal_route_prefixes = Vec::new();
            let mut native_php_functions = Vec::new();
            let mut decorators = Vec::new();
            let mut php_classes = Vec::new();
            let mut php_interfaces = Vec::new();
            let mut php_enums = Vec::new();
            let mut php_attributes = Vec::new();
            let mut php_functions = Vec::new();
            let mut core_flags = std::collections::HashMap::new();
            let ctx = build_ctx(
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
            ($body)(&ctx)
        }};
    }

    #[test]
    fn shared_bool_error_tag_uses_actual_var_name() {
        // When the operator sets `OX_SHARED_*` (plugin-prefixed fallback)
        // the error must name *that* variable, not the `SHARED_*` form they
        // never touched.
        let vars = [
            ("SHARED_TEST_FLAG", None),
            ("TEST_FLAG", None),
            ("OX_SHARED_TEST_FLAG", Some("garbage")),
        ];
        let err = crate::config::test_env::with_env(&vars, || {
            with_ctx!(|ctx: &PluginContext| {
                shared_bool(ctx, "TEST_FLAG", false).expect_err("garbage must error")
            })
        });

        let msg = err.to_string();
        assert!(
            msg.contains("OX_SHARED_TEST_FLAG"),
            "error must name the actual env var, got: {msg}"
        );
    }

    #[derive(Clone, Default)]
    struct Captured(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
        type Writer = Captured;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Run `SharedConfig::from_ctx` with `var=value` as the only spelling of
    /// `key` set, and return what it logged.
    fn from_ctx_log_with(key: &str, var: &str, value: &str) -> String {
        let public = format!("SHARED_{key}");
        let prefixed = format!("OX_SHARED_{key}");
        let vars: Vec<(&str, Option<&str>)> = [public.as_str(), prefixed.as_str(), key]
            .into_iter()
            .map(|n| (n, (n == var).then_some(value)))
            .collect();

        let captured = Captured::default();
        let subscriber = tracing_subscriber::fmt()
            .with_writer(captured.clone())
            .with_max_level(tracing::Level::WARN)
            .with_ansi(false)
            .finish();
        crate::config::test_env::with_env(&vars, || {
            tracing::subscriber::with_default(subscriber, || {
                with_ctx!(|ctx: &PluginContext| SharedConfig::from_ctx(ctx).expect("config"))
            })
        });

        let bytes = captured.0.lock().unwrap().clone();
        String::from_utf8(bytes).expect("utf-8 log")
    }

    /// None of these settings drives anything, so a deployment still carrying
    /// one is told so at startup, under whichever of the two documented
    /// spellings it used — and not under the bare key, which may belong to
    /// other software.
    #[test]
    fn deprecated_settings_are_reported_as_ignored() {
        for (key, value) in [
            ("SOFT_LIMIT_RATIO", "0.5"),
            ("SHUTDOWN_TIMEOUT_SECONDS", "5"),
            ("POISON_STRICT", "true"),
        ] {
            for var in [format!("SHARED_{key}"), format!("OX_SHARED_{key}")] {
                let log = from_ctx_log_with(key, &var, value);
                assert!(
                    log.contains("WARN")
                        && log.contains(&format!("{var} is deprecated and ignored")),
                    "{var} must log a deprecation WARN, got: {log}"
                );
            }
            let log = from_ctx_log_with(key, key, value);
            assert!(
                !log.contains(&format!("{key} is deprecated")),
                "bare {key} must not be reported, got: {log}"
            );
        }
    }
}
