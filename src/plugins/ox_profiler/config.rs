//! Profiler configuration parsed from `PROFILER_*` env vars at plugin init.
//!
//! The full field set covers everything spec §9 enumerates so that the
//! trigger layer and downstream consumers (storage, export, internal routes)
//! can read from a stable config surface.

use std::path::PathBuf;
use std::sync::Arc;

use crate::config::{parse_bool_opt, parse_bool_strict};
use crate::plugin::{PluginContext, PluginError};

/// Profiler plugin configuration. Populated once at `ProfilerPlugin::init` via
/// [`ProfilerConfig::from_ctx`]. Immutable afterwards.
#[derive(Debug, Clone)]
pub struct ProfilerConfig {
    pub enabled: bool,
    pub auth_token: Option<Arc<str>>,
    pub sample_rate: f64,
    pub internal: bool,
    pub max_spans: u32,
    pub max_depth: u16,
    pub output_dir: PathBuf,
    pub output_formats: Vec<String>,
    pub disk_max_per_sec: u32,
    pub retention_count: u32,
    pub export_url: Option<Arc<str>>,
    pub export_format: String,
    pub export_auth_token: Option<Arc<str>>,
    pub export_xhgui: bool,
}

impl Default for ProfilerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            auth_token: None,
            sample_rate: 0.0,
            internal: false,
            max_spans: 50_000,
            max_depth: 256,
            output_dir: PathBuf::from("/tmp/oxphp-profiles"),
            output_formats: vec!["xhprof".into(), "speedscope".into()],
            disk_max_per_sec: 10,
            retention_count: 100,
            export_url: None,
            export_format: "xhprof".into(),
            export_auth_token: None,
            export_xhgui: false,
        }
    }
}

impl ProfilerConfig {
    /// Parse `PROFILER_*` config from the plugin context.
    ///
    /// Every field is parsed unconditionally — even when `enabled=false` —
    /// so a typo like `PROFILER_INTERNAL=ture` surfaces at startup instead
    /// of waiting until the operator flips `PROFILER_ENABLED=true` in prod.
    pub fn from_ctx(ctx: &PluginContext) -> Result<Self, PluginError> {
        let enabled = parse_bool_opt("PROFILER_ENABLED", ctx.config("ENABLED").as_deref(), false)
            .map_err(|e| PluginError::Config(e.to_string()))?;

        let auth_token = ctx.config("AUTH_TOKEN").and_then(|v| {
            if v.is_empty() {
                None
            } else {
                Some(Arc::<str>::from(v))
            }
        });

        let sample_rate = parse_f64(ctx.config("SAMPLE_RATE").as_deref(), 0.0).clamp(0.0, 1.0);
        let internal = parse_bool_opt(
            "PROFILER_INTERNAL",
            ctx.config("INTERNAL").as_deref(),
            false,
        )
        .map_err(|e| PluginError::Config(e.to_string()))?;
        let max_spans = parse_u32(ctx.config("MAX_SPANS").as_deref(), 50_000);
        let max_depth =
            parse_u32(ctx.config("MAX_DEPTH").as_deref(), 256).min(u16::MAX as u32) as u16;

        let output_dir = ctx
            .config("OUTPUT_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp/oxphp-profiles"));

        let output_formats = ctx
            .config("OUTPUT_FORMATS")
            .unwrap_or_else(|| "xhprof,speedscope".to_string())
            .split(',')
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect();

        let disk_max_per_sec = parse_u32(ctx.config("DISK_MAX_PER_SEC").as_deref(), 10);
        let retention_count = parse_u32(ctx.config("RETENTION_COUNT").as_deref(), 100);

        let export_url = ctx.config("EXPORT_URL").and_then(|v| {
            if v.is_empty() {
                None
            } else {
                Some(Arc::<str>::from(v))
            }
        });
        let export_format = ctx
            .config("EXPORT_FORMAT")
            .unwrap_or_else(|| "xhprof".to_string())
            .to_lowercase();
        let export_auth_token = ctx.config("EXPORT_AUTH_TOKEN").and_then(|v| {
            if v.is_empty() {
                None
            } else {
                Some(Arc::<str>::from(v))
            }
        });

        // Tri-state: explicit truthy/falsy wins, unset/empty falls through to
        // URL-based auto-detection. Garbage values surface as a startup error.
        let export_xhgui = match ctx.config("EXPORT_XHGUI").as_deref() {
            None | Some("") => export_url
                .as_deref()
                .map(|u| u.contains("xhgui") || u.ends_with("/run/import"))
                .unwrap_or(false),
            Some(v) => parse_bool_strict(v)
                .map_err(|e| PluginError::Config(format!("PROFILER_EXPORT_XHGUI: {e}")))?,
        };

        Ok(Self {
            enabled,
            auth_token,
            sample_rate,
            internal,
            max_spans,
            max_depth,
            output_dir,
            output_formats,
            disk_max_per_sec,
            retention_count,
            export_url,
            export_format,
            export_auth_token,
            export_xhgui,
        })
    }
}

fn parse_f64(s: Option<&str>, default: f64) -> f64 {
    s.and_then(|v| v.parse().ok()).unwrap_or(default)
}

fn parse_u32(s: Option<&str>, default: u32) -> u32 {
    s.and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_when_disabled() {
        let cfg = ProfilerConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.sample_rate, 0.0);
        assert!(cfg.auth_token.is_none());
        assert_eq!(cfg.max_spans, 50_000);
        assert_eq!(cfg.max_depth, 256);
        assert_eq!(cfg.output_formats, vec!["xhprof", "speedscope"]);
        assert_eq!(cfg.retention_count, 100);
    }

    #[test]
    fn test_parse_u32_helper() {
        assert_eq!(parse_u32(Some("42"), 100), 42);
        assert_eq!(parse_u32(Some("not-a-number"), 100), 100);
        assert_eq!(parse_u32(None, 100), 100);
    }

    #[test]
    fn test_parse_f64_helper_clamp_not_here() {
        // Clamping is applied at call sites (e.g. sample_rate). The helper itself doesn't clamp.
        assert_eq!(parse_f64(Some("0.5"), 0.0), 0.5);
        assert_eq!(parse_f64(Some("2.0"), 0.0), 2.0);
        assert_eq!(parse_f64(None, 0.1), 0.1);
    }
}
