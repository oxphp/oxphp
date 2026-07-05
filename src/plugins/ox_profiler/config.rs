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

use super::storage::OutputFormat;

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
    /// Wrap xhprof pushes in Buggregator's `/api/profiler/store` envelope
    /// (`{profile, tags, app_name, hostname, date}`). Mutually exclusive
    /// with `export_xhgui`. Auto-detected from a `…/api/profiler/store`
    /// export URL; overridable via `PROFILER_EXPORT_BUGGREGATOR`.
    pub export_buggregator: bool,
    /// `app_name` for the Buggregator envelope (project grouping). Unused
    /// by other envelopes.
    pub export_app_name: Option<Arc<str>>,
    /// `tags` for the Buggregator envelope, parsed from
    /// `PROFILER_EXPORT_TAGS` (`key=value,key2=value2`).
    pub export_tags: Vec<(String, String)>,
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
            export_buggregator: false,
            export_app_name: None,
            export_tags: Vec::new(),
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

        // Single source of truth for the xhprof push envelope: both tri-state
        // knobs and the URL are resolved together so xhgui/buggregator
        // detection cannot drift or both activate. See `resolve_export_envelope`.
        let xhgui_raw = ctx.config("EXPORT_XHGUI");
        let envelope = resolve_export_envelope(
            xhgui_raw.as_deref(),
            ctx.config("EXPORT_BUGGREGATOR").as_deref(),
            export_url.as_deref(),
        )?;
        let export_xhgui = envelope == ExportEnvelope::Xhgui;
        let export_buggregator = envelope == ExportEnvelope::Buggregator;

        // The xhgui and Buggregator envelopes are xhprof-based and always emit
        // their xhprof body — the pusher ignores PROFILER_EXPORT_FORMAT for
        // them. A non-xhprof format alongside an active envelope is therefore
        // inert, not fatal: warn so the operator knows the knob has no effect,
        // but never crash the whole server over an optional export's config.
        // `from_str_opt` (the pusher's own parser) also matches the
        // `xhprof.json` alias, so that spelling doesn't trip the warning.
        if (export_xhgui || export_buggregator)
            && OutputFormat::from_str_opt(&export_format) != Some(OutputFormat::Xhprof)
        {
            let envelope = if export_buggregator {
                "Buggregator"
            } else {
                "xhgui"
            };
            tracing::warn!(
                plugin = "profiler",
                format = %export_format,
                "PROFILER_EXPORT_FORMAT is ignored while the {envelope} export envelope is \
                 active; the envelope always emits xhprof"
            );
        }

        // A resolved envelope that contradicts a specific-endpoint URL (xhgui
        // aimed at a Buggregator store path, or vice versa) will be rejected by
        // the receiver. This only happens when an explicit flag overrides the
        // URL's own signal — warn rather than lose profiles silently.
        if let Some(url) = export_url.as_deref() {
            if export_xhgui && is_buggregator_store_url(url) {
                tracing::warn!(
                    plugin = "profiler",
                    "PROFILER_EXPORT_XHGUI is set but PROFILER_EXPORT_URL is a Buggregator store \
                     endpoint; the xhgui envelope will likely be rejected"
                );
            } else if export_buggregator && is_xhgui_import_url(url) {
                tracing::warn!(
                    plugin = "profiler",
                    "PROFILER_EXPORT_BUGGREGATOR is set but PROFILER_EXPORT_URL looks like an xhgui \
                     import endpoint; the Buggregator envelope will likely be rejected"
                );
            }
        }

        // An enabled envelope with no target URL pushes nothing — and without
        // this warning gives no signal at all: the pusher is only built when a
        // URL is present, and the app_name/tags warning below is gated on Raw.
        if export_url.is_none() && (export_xhgui || export_buggregator) {
            tracing::warn!(
                plugin = "profiler",
                "an export envelope is enabled but PROFILER_EXPORT_URL is empty; HTTP push is \
                 disabled and no profiles will be sent"
            );
        }

        // Upgrade guard: earlier versions auto-wrapped any URL merely containing
        // the `xhgui` substring. Detection is now endpoint-path-only, so such a
        // URL that does not end in /run/import silently falls back to raw. Warn
        // when the knob is unset, so an operator upgrading isn't left pushing a
        // raw body an xhgui importer rejects, without turning the fuzzy
        // auto-detect back on.
        let xhgui_unset = matches!(xhgui_raw.as_deref().map(str::trim), None | Some(""));
        if envelope == ExportEnvelope::None
            && xhgui_unset
            && export_url.as_deref().is_some_and(|u| u.contains("xhgui"))
        {
            tracing::warn!(
                plugin = "profiler",
                "PROFILER_EXPORT_URL mentions `xhgui` but its path does not end in /run/import and \
                 PROFILER_EXPORT_XHGUI is unset; pushing raw xhprof (an xhgui importer rejects it) \
                 — set PROFILER_EXPORT_XHGUI=true if this is an xhgui target"
            );
        }

        let export_app_name = ctx.config("EXPORT_APP_NAME").and_then(|v| {
            // Trim like the tri-state/tag parsers, so a `${APP_NAME:- }`
            // substitution doesn't group every profile under a blank project.
            let v = v.trim();
            if v.is_empty() {
                None
            } else {
                Some(Arc::<str>::from(v))
            }
        });

        let export_tags = parse_export_tags(ctx.config("EXPORT_TAGS").as_deref())?;

        // app_name/tags only feed the Buggregator envelope. If set under any
        // other envelope they are silently inert — warn so the operator isn't
        // left believing their grouping/filtering config took effect.
        if !export_buggregator && (export_app_name.is_some() || !export_tags.is_empty()) {
            tracing::warn!(
                plugin = "profiler",
                "PROFILER_EXPORT_APP_NAME / PROFILER_EXPORT_TAGS are set but the export envelope \
                 is not Buggregator; they are ignored"
            );
        }

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
            export_buggregator,
            export_app_name,
            export_tags,
            exclude_paths,
            exclude_patterns,
        })
    }
}

/// The xhprof push envelope chosen for `PROFILER_EXPORT_URL`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ExportEnvelope {
    None,
    Xhgui,
    Buggregator,
}

/// Resolve the xhprof push envelope from the two tri-state knobs
/// (`PROFILER_EXPORT_XHGUI`, `PROFILER_EXPORT_BUGGREGATOR`) and the export URL,
/// in one place so the two can never both activate or their detection drift.
///
/// Precedence:
/// - An explicit truthy value wins over any auto-detect, so operator intent is
///   never overridden by a URL that merely looks like the other envelope.
///   Both explicitly true is a config error — and only then does the message
///   name both variables, since the operator actually set both.
/// - Otherwise auto-detect from the URL. Buggregator's ingest is matched on the
///   *path* ending in `/api/profiler/store` (query/fragment stripped) — a far
///   more specific signal than xhgui's historical `xhgui` substring, and it is
///   checked first, so a store URL whose host merely contains `xhgui` still
///   selects Buggregator. An explicit `false` hard-disables that side's
///   auto-detect.
fn resolve_export_envelope(
    xhgui_raw: Option<&str>,
    bugg_raw: Option<&str>,
    url: Option<&str>,
) -> Result<ExportEnvelope, PluginError> {
    let xhgui = parse_tristate(xhgui_raw, "PROFILER_EXPORT_XHGUI")?;
    let bugg = parse_tristate(bugg_raw, "PROFILER_EXPORT_BUGGREGATOR")?;

    if xhgui == Some(true) && bugg == Some(true) {
        return Err(PluginError::Config(
            "PROFILER_EXPORT_XHGUI and PROFILER_EXPORT_BUGGREGATOR are both enabled; they are \
             mutually exclusive — enable only one xhprof export envelope"
                .into(),
        ));
    }
    // Explicit truthy wins over any auto-detect (incl. a URL that looks like
    // the other envelope).
    if bugg == Some(true) {
        return Ok(ExportEnvelope::Buggregator);
    }
    if xhgui == Some(true) {
        return Ok(ExportEnvelope::Xhgui);
    }
    // Auto-detect. The store path is the more specific match, checked first;
    // an explicit `false` disables that side.
    if bugg != Some(false) && url.map(is_buggregator_store_url).unwrap_or(false) {
        return Ok(ExportEnvelope::Buggregator);
    }
    if xhgui != Some(false) && url.map(is_xhgui_import_url).unwrap_or(false) {
        return Ok(ExportEnvelope::Xhgui);
    }
    Ok(ExportEnvelope::None)
}

/// Tri-state parse of an envelope knob: unset/empty/whitespace → `None`
/// (auto-detect), otherwise a strict boolean whose garbage value is a startup
/// error. Trimming matches the codebase-wide treatment of `${VAR:- }`-style
/// substitutions as unset (so a stray space doesn't crash init).
fn parse_tristate(raw: Option<&str>, var: &str) -> Result<Option<bool>, PluginError> {
    match raw.map(str::trim) {
        None | Some("") => Ok(None),
        Some(v) => parse_bool_strict(v)
            .map(Some)
            .map_err(|e| PluginError::Config(format!("{var}: {e}"))),
    }
}

/// The URL's path, with any `?query` / `#fragment` stripped.
fn url_path(u: &str) -> &str {
    u.split(['?', '#']).next().unwrap_or(u)
}

/// True when the URL's path is Buggregator's profiler ingest endpoint. Matched
/// as a path suffix (not an anywhere-substring) so an unrelated collector that
/// merely mentions the string elsewhere — or a longer path under it — is not
/// silently wrapped.
fn is_buggregator_store_url(u: &str) -> bool {
    url_path(u)
        .trim_end_matches('/')
        .ends_with("/api/profiler/store")
}

/// True for xhgui's canonical import endpoint: a path ending in `/run/import`.
///
/// Earlier revisions also matched a fuzzy `xhgui` substring anywhere in the URL
/// (host/path/query). That heuristic proved unwinnable — reviewers disagreed on
/// whether `?app=xhgui` should wrap or not, and it cross-contaminated with the
/// Buggregator store path — so it is dropped: auto-detection keys **only** on
/// each tool's canonical endpoint path (`/run/import` for xhgui,
/// `/api/profiler/store` for Buggregator), never on host or query. Pointing at
/// a non-standard xhgui path? Set `PROFILER_EXPORT_XHGUI=true` explicitly.
fn is_xhgui_import_url(u: &str) -> bool {
    url_path(u).trim_end_matches('/').ends_with("/run/import")
}

/// Parse `PROFILER_EXPORT_TAGS` — a comma-separated `key=value` list —
/// into ordered pairs for the Buggregator envelope. Empty/unset yields no
/// tags. A token without `=` (or with an empty key) is a config error, so
/// a typo surfaces at startup rather than shipping unlabeled profiles.
fn parse_export_tags(raw: Option<&str>) -> Result<Vec<(String, String)>, PluginError> {
    let raw = match raw {
        Some(s) if !s.trim().is_empty() => s,
        _ => return Ok(Vec::new()),
    };
    let mut out = Vec::new();
    for tok in raw.split(',') {
        let tok = tok.trim();
        if tok.is_empty() {
            continue;
        }
        match tok.split_once('=') {
            Some((k, v)) => {
                let k = k.trim();
                if k.is_empty() {
                    return Err(PluginError::Config(format!(
                        "PROFILER_EXPORT_TAGS: empty key in `{tok}`"
                    )));
                }
                // Serializing to a JSON object would silently drop all but the
                // last value for a repeated key — reject it at startup instead.
                if out.iter().any(|(ek, _)| ek == k) {
                    return Err(PluginError::Config(format!(
                        "PROFILER_EXPORT_TAGS: duplicate key `{k}`"
                    )));
                }
                out.push((k.to_string(), v.trim().to_string()));
            }
            None => {
                return Err(PluginError::Config(format!(
                    "PROFILER_EXPORT_TAGS: `{tok}` is not `key=value`"
                )));
            }
        }
    }
    Ok(out)
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

    use ExportEnvelope::{Buggregator, None as EnvNone, Xhgui};

    #[test]
    fn test_envelope_autodetect_from_store_url() {
        assert_eq!(
            resolve_export_envelope(
                None,
                None,
                Some("http://buggregator:8000/api/profiler/store")
            )
            .unwrap(),
            Buggregator
        );
        assert_eq!(
            resolve_export_envelope(None, None, Some("http://collector/run/import")).unwrap(),
            Xhgui
        );
        assert_eq!(
            resolve_export_envelope(None, None, Some("http://collector/ingest")).unwrap(),
            EnvNone
        );
        assert_eq!(resolve_export_envelope(None, None, None).unwrap(), EnvNone);
    }

    #[test]
    fn test_store_path_beats_xhgui_substring() {
        // Host contains "xhgui" but the path is the Buggregator store endpoint:
        // the specific path signal must win, else the xhgui envelope is sent to
        // a Buggregator endpoint and every profile is lost.
        assert_eq!(
            resolve_export_envelope(
                None,
                None,
                Some("http://xhgui-obs.internal/api/profiler/store")
            )
            .unwrap(),
            Buggregator
        );
    }

    #[test]
    fn test_store_match_is_a_narrow_path_suffix() {
        // Query string is ignored (still matches).
        assert_eq!(
            resolve_export_envelope(None, None, Some("http://b/api/profiler/store?token=x"))
                .unwrap(),
            Buggregator
        );
        // A longer path *under* the endpoint is NOT the endpoint — avoids
        // silently wrapping an unrelated collector at a nested path.
        assert_eq!(
            resolve_export_envelope(None, None, Some("http://b/api/profiler/store/v2")).unwrap(),
            EnvNone
        );
        // Substring in the query only (not the path) must not match.
        assert_eq!(
            resolve_export_envelope(None, None, Some("http://b/ingest?to=/api/profiler/store"))
                .unwrap(),
            EnvNone
        );
    }

    #[test]
    fn test_xhgui_autodetect_is_endpoint_only() {
        // Only the canonical /run/import endpoint path auto-selects xhgui.
        assert_eq!(
            resolve_export_envelope(None, None, Some("http://collector/run/import")).unwrap(),
            Xhgui
        );
        // "xhgui" in the host or query is NOT enough — that fuzzy heuristic was
        // dropped; the operator sets PROFILER_EXPORT_XHGUI=true for such URLs.
        assert_eq!(
            resolve_export_envelope(None, None, Some("http://xhgui.internal/import")).unwrap(),
            EnvNone
        );
        assert_eq!(
            resolve_export_envelope(None, None, Some("http://collector/ingest?team=xhgui"))
                .unwrap(),
            EnvNone
        );
    }

    #[test]
    fn test_buggregator_false_on_xhgui_host_store_url_is_raw() {
        // Host carries "xhgui" (an xhgui→Buggregator migration kept the name),
        // path is the store endpoint, buggregator explicitly disabled. Must
        // resolve to Raw — honoring BUGGREGATOR=false — not fall through to an
        // xhgui envelope aimed at a Buggregator endpoint (the dropped fuzzy
        // `contains("xhgui")` used to do exactly that).
        assert_eq!(
            resolve_export_envelope(
                None,
                Some("false"),
                Some("http://xhgui-prod/api/profiler/store")
            )
            .unwrap(),
            EnvNone
        );
    }

    #[test]
    fn test_explicit_wins_over_autodetect() {
        // Explicit buggregator=true wins even when the URL looks like xhgui —
        // and does NOT raise the mutual-exclusion error (that's for both-set).
        assert_eq!(
            resolve_export_envelope(None, Some("true"), Some("http://x/run/import")).unwrap(),
            Buggregator
        );
        // Explicit xhgui=true wins on a store URL.
        assert_eq!(
            resolve_export_envelope(Some("true"), None, Some("http://b/api/profiler/store"))
                .unwrap(),
            Xhgui
        );
        // Explicit false hard-disables that side's auto-detect.
        assert_eq!(
            resolve_export_envelope(None, Some("false"), Some("http://b/api/profiler/store"))
                .unwrap(),
            EnvNone
        );
    }

    #[test]
    fn test_both_explicit_true_is_error() {
        assert!(resolve_export_envelope(Some("true"), Some("true"), None).is_err());
    }

    #[test]
    fn test_xhprof_json_alias_recognized_as_xhprof() {
        // The "format is ignored" warning keys on OutputFormat::from_str_opt,
        // which maps the `.json` alias to Xhprof — so `xhprof.json` alongside an
        // envelope must not trip a spurious warning.
        assert_eq!(
            OutputFormat::from_str_opt("xhprof.json"),
            Some(OutputFormat::Xhprof)
        );
        assert_eq!(
            OutputFormat::from_str_opt("xhprof"),
            Some(OutputFormat::Xhprof)
        );
        assert_ne!(
            OutputFormat::from_str_opt("speedscope"),
            Some(OutputFormat::Xhprof)
        );
    }

    #[test]
    fn test_tristate_whitespace_is_unset_not_error() {
        // A `${VAR:- }`-style stray space must fall through to auto-detect, not
        // crash init via the strict bool parser.
        assert_eq!(parse_tristate(Some("  "), "X").unwrap(), None);
        assert_eq!(
            resolve_export_envelope(Some(" "), Some(" "), Some("http://b/api/profiler/store"))
                .unwrap(),
            Buggregator
        );
    }

    #[test]
    fn test_envelope_invalid_bool_errors() {
        assert!(resolve_export_envelope(None, Some("ture"), None).is_err());
        assert!(resolve_export_envelope(Some("nope"), None, None).is_err());
    }

    #[test]
    fn test_export_tags_unset_is_empty() {
        assert!(parse_export_tags(None).unwrap().is_empty());
        assert!(parse_export_tags(Some("")).unwrap().is_empty());
        assert!(parse_export_tags(Some("   ")).unwrap().is_empty());
    }

    #[test]
    fn test_export_tags_parses_pairs_in_order() {
        let tags = parse_export_tags(Some("env=prod, region=eu-1 ,tier=web")).unwrap();
        assert_eq!(
            tags,
            vec![
                ("env".to_string(), "prod".to_string()),
                ("region".to_string(), "eu-1".to_string()),
                ("tier".to_string(), "web".to_string()),
            ]
        );
    }

    #[test]
    fn test_export_tags_allows_empty_value() {
        let tags = parse_export_tags(Some("debug=")).unwrap();
        assert_eq!(tags, vec![("debug".to_string(), String::new())]);
    }

    #[test]
    fn test_export_tags_rejects_token_without_eq() {
        assert!(parse_export_tags(Some("env=prod,oops")).is_err());
    }

    #[test]
    fn test_export_tags_rejects_empty_key() {
        assert!(parse_export_tags(Some("=value")).is_err());
    }

    #[test]
    fn test_export_tags_rejects_duplicate_key() {
        // Would silently collapse to last-wins in a JSON object — reject it.
        assert!(parse_export_tags(Some("env=a,env=b")).is_err());
    }
}
