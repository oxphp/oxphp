//! Profiler configuration parsed from `PROFILER_*` env vars at plugin init.
//!
//! The full field set covers everything spec §9 enumerates so that the
//! trigger layer and downstream consumers (storage, export, internal routes)
//! can read from a stable config surface.

use std::path::PathBuf;
use std::sync::Arc;

use globset::GlobSet;

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
    /// Compiled glob set from `PROFILER_EXCLUDE_PATHS`. `None` = no exclusions.
    /// Matched paths are skipped by `sample_rate` activation only; explicit
    /// triggers still profile them.
    pub exclude_paths: Option<Arc<GlobSet>>,
    /// Source patterns behind `exclude_paths`, surfaced in `/config` so an
    /// operator can verify the exclusion compiled. Empty when none configured.
    pub exclude_patterns: Vec<String>,
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
            exclude_paths: None,
            exclude_patterns: Vec::new(),
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

        let (exclude_paths, exclude_patterns) =
            match parse_exclude_paths(ctx.config("EXCLUDE_PATHS").as_deref())? {
                Some((set, patterns)) => (Some(set), patterns),
                None => (None, Vec::new()),
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
            exclude_paths,
            exclude_patterns,
        })
    }
}

/// Parse `PROFILER_EXCLUDE_PATHS` into a compiled `GlobSet` plus its source
/// patterns. Returns `Ok(None)` when unset or empty after trimming. Glob
/// compilation is shared with `PHP_DENY_PATHS` via [`crate::config::compile_glob_csv`]
/// so the two cannot drift apart in syntax.
#[allow(clippy::type_complexity)]
fn parse_exclude_paths(
    raw: Option<&str>,
) -> Result<Option<(Arc<GlobSet>, Vec<String>)>, PluginError> {
    let raw = match raw {
        Some(s) => s,
        None => return Ok(None),
    };
    match crate::config::compile_glob_csv(raw, "PROFILER_EXCLUDE_PATHS")
        .map_err(PluginError::Config)?
    {
        Some((set, patterns)) => Ok(Some((Arc::new(set), patterns))),
        None => Ok(None),
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

    #[test]
    fn test_exclude_paths_unset_is_none() {
        assert!(parse_exclude_paths(None).unwrap().is_none());
        assert!(parse_exclude_paths(Some("")).unwrap().is_none());
        assert!(parse_exclude_paths(Some("  ,  ")).unwrap().is_none());
    }

    #[test]
    fn test_exclude_paths_glob_semantics() {
        // Patterns match against the URI path with leading '/' stripped.
        let (set, patterns) = parse_exclude_paths(Some("/_profiler,/_profiler/**,/_wdt/**"))
            .unwrap()
            .expect("some");
        assert!(set.is_match("_profiler")); // bare path, covered by "/_profiler"
        assert!(set.is_match("_profiler/abc123")); // subtree, covered by "/_profiler/**"
        assert!(set.is_match("_wdt/token")); // "/_wdt/**"
        assert!(!set.is_match("_profilerx")); // not a prefix match
        assert!(!set.is_match("api/users"));
        // Patterns are returned normalized (leading '/' stripped) for /config.
        assert_eq!(patterns, vec!["_profiler", "_profiler/**", "_wdt/**"]);
    }

    #[test]
    fn test_exclude_paths_single_star_does_not_cross_slash() {
        let (set, _) = parse_exclude_paths(Some("/_wdt/*")).unwrap().expect("some");
        assert!(set.is_match("_wdt/token"));
        assert!(!set.is_match("_wdt/a/b"));
    }

    #[test]
    fn test_exclude_paths_invalid_glob_errors() {
        assert!(parse_exclude_paths(Some("/_wdt/[abc")).is_err());
    }
}
