//! SHARED_* env var reader. Uses PluginContext::config_prefixed to
//! avoid collision with other plugins' bare keys.

use crate::plugin::PluginContext;

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
    pub fn from_ctx(ctx: &PluginContext) -> Self {
        Self {
            enabled: parse_bool(shared_env(ctx, "ENABLED"), true),
            max_entries: parse_usize(shared_env(ctx, "MAX_ENTRIES"), 100_000),
            max_bytes: parse_u64(shared_env(ctx, "MAX_BYTES"), 1_073_741_824),
            soft_limit_ratio: parse_f32(shared_env(ctx, "SOFT_LIMIT_RATIO"), 0.7),
            metrics_enabled: parse_bool(shared_env(ctx, "METRICS_ENABLED"), true),
            introspection_enabled: parse_bool(shared_env(ctx, "INTROSPECTION_ENABLED"), true),
            introspection_preview_enabled: parse_bool(
                shared_env(ctx, "INTROSPECTION_PREVIEW_ENABLED"),
                true,
            ),
            cycle_detect_depth: parse_usize(shared_env(ctx, "CYCLE_DETECT_DEPTH"), 16),
            cycle_detect_edges: parse_usize(shared_env(ctx, "CYCLE_DETECT_EDGES"), 10_000),
            shutdown_timeout_seconds: parse_f32(shared_env(ctx, "SHUTDOWN_TIMEOUT_SECONDS"), 5.0),
            poison_strict: parse_bool(shared_env(ctx, "POISON_STRICT"), false),
            lock_diagnostics: parse_lock_diag(shared_env(ctx, "LOCK_DIAGNOSTICS")),
            lock_poll_interval_ms: parse_u64(shared_env(ctx, "LOCK_POLL_INTERVAL_MS"), 100),
            preview_string_limit: parse_usize(shared_env(ctx, "PREVIEW_STRING_LIMIT"), 256),
            preview_array_limit: parse_usize(shared_env(ctx, "PREVIEW_ARRAY_LIMIT"), 20),
        }
    }
}

/// Read a Shared-plugin env var, trying three lookups in priority order:
///   1. `SHARED_{KEY}`          — documented public API
///   2. `OX_SHARED_{KEY}`       — plugin-prefixed fallback
///   3. `{KEY}`                 — bare key last resort
fn shared_env(ctx: &PluginContext, key: &str) -> Option<String> {
    std::env::var(format!("SHARED_{key}"))
        .ok()
        .or_else(|| ctx.config_prefixed(key))
        .or_else(|| std::env::var(key).ok())
}

fn parse_bool(val: Option<String>, default: bool) -> bool {
    match val.as_deref() {
        Some("0") | Some("false") | Some("no") | Some("off") => false,
        Some(_) => true,
        None => default,
    }
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
    fn parse_bool_defaults() {
        assert!(parse_bool(None, true));
        assert!(!parse_bool(None, false));
        assert!(!parse_bool(Some("0".into()), true));
        assert!(parse_bool(Some("1".into()), false));
        assert!(!parse_bool(Some("false".into()), true));
    }

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
}
