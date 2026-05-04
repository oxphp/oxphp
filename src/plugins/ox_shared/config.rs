//! SHARED_* env var reader. Uses PluginContext::config_prefixed to
//! avoid collision with other plugins' bare keys.

use crate::config::parse_bool_opt;
use crate::plugin::{PluginContext, PluginError};

#[derive(Debug, Clone)]
pub struct SharedConfig {
    pub enabled: bool,
    pub max_entries: usize,
    pub max_bytes: u64,
    pub soft_limit_ratio: f32,
    pub metrics_enabled: bool,
    pub introspection_enabled: bool,
    pub introspection_preview_enabled: bool,
    pub cycle_detect_depth: usize,
    pub cycle_detect_edges: usize,
    pub shutdown_timeout_seconds: f32,
    pub poison_strict: bool,
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
        Ok(Self {
            enabled: shared_bool(ctx, "ENABLED", true)?,
            max_entries: parse_usize(shared_value(ctx, "MAX_ENTRIES"), 100_000),
            max_bytes: parse_u64(shared_value(ctx, "MAX_BYTES"), 1_073_741_824),
            soft_limit_ratio: parse_f32(shared_value(ctx, "SOFT_LIMIT_RATIO"), 0.7),
            metrics_enabled: shared_bool(ctx, "METRICS_ENABLED", true)?,
            introspection_enabled: shared_bool(ctx, "INTROSPECTION_ENABLED", true)?,
            introspection_preview_enabled: shared_bool(ctx, "INTROSPECTION_PREVIEW_ENABLED", true)?,
            cycle_detect_depth: parse_usize(shared_value(ctx, "CYCLE_DETECT_DEPTH"), 16),
            cycle_detect_edges: parse_usize(shared_value(ctx, "CYCLE_DETECT_EDGES"), 10_000),
            shutdown_timeout_seconds: parse_f32(shared_value(ctx, "SHUTDOWN_TIMEOUT_SECONDS"), 5.0),
            poison_strict: shared_bool(ctx, "POISON_STRICT", false)?,
            lock_diagnostics: parse_lock_diag(shared_value(ctx, "LOCK_DIAGNOSTICS")),
            lock_poll_interval_ms: parse_u64(shared_value(ctx, "LOCK_POLL_INTERVAL_MS"), 100),
            preview_string_limit: parse_usize(shared_value(ctx, "PREVIEW_STRING_LIMIT"), 256),
            preview_array_limit: parse_usize(shared_value(ctx, "PREVIEW_ARRAY_LIMIT"), 20),
        })
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

fn parse_f32(val: Option<String>, default: f32) -> f32 {
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

    /// Locks env-mutating tests so they don't race with each other.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev_public = std::env::var("SHARED_TEST_FLAG").ok();
        let prev_prefixed = std::env::var("OX_SHARED_TEST_FLAG").ok();
        let prev_bare = std::env::var("TEST_FLAG").ok();
        std::env::remove_var("SHARED_TEST_FLAG");
        std::env::remove_var("TEST_FLAG");
        std::env::set_var("OX_SHARED_TEST_FLAG", "garbage");

        let err = with_ctx!(|ctx: &PluginContext| {
            shared_bool(ctx, "TEST_FLAG", false).expect_err("garbage must error")
        });

        // Restore env before asserting.
        for (name, prev) in [
            ("SHARED_TEST_FLAG", prev_public),
            ("OX_SHARED_TEST_FLAG", prev_prefixed),
            ("TEST_FLAG", prev_bare),
        ] {
            match prev {
                Some(v) => std::env::set_var(name, v),
                None => std::env::remove_var(name),
            }
        }

        let msg = err.to_string();
        assert!(
            msg.contains("OX_SHARED_TEST_FLAG"),
            "error must name the actual env var, got: {msg}"
        );
    }
}
