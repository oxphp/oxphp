//! Trigger logic — decide whether an incoming request should be profiled.
//!
//! `should_profile` is a pure function: it inspects a `PluginRequestView`
//! against the configured `ProfilerConfig` and returns
//! `Some(ActivationDecision)` if the request meets the activation criteria
//! (header / cookie / query-string token match, or random sampling hit).
//! `None` means "run at ApmOnly or Off as usual".

use rand::{Rng, RngExt};
use subtle::ConstantTimeEq;

use crate::plugin::handler::PluginRequestView;
use crate::profiling::ProfilingMode;

use super::config::ProfilerConfig;

/// Why profiling was activated for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ActivationSource {
    Header,
    Cookie,
    Query,
    SampleRate,
}

/// The outcome of `should_profile` when activation succeeds.
#[derive(Debug, Clone)]
pub struct ActivationDecision {
    pub source: ActivationSource,
    pub mode: ProfilingMode,
    pub run_id: String,
}

/// Decide whether to profile this request.
///
/// Returns `None` when the profiler is disabled globally or the request
/// carries no valid trigger.
pub fn should_profile<R: Rng + ?Sized>(
    req: &PluginRequestView,
    cfg: &ProfilerConfig,
    rng: &mut R,
) -> Option<ActivationDecision> {
    if !cfg.enabled {
        return None;
    }

    // Explicit activation: priority header > cookie > query.
    if let Some(src) = check_explicit(req, cfg) {
        return Some(ActivationDecision {
            source: src,
            mode: ProfilingMode::ProfileAll,
            run_id: generate_run_id(req, rng),
        });
    }

    // Random sampling: no token required. Excluded paths are never sampled
    // (short-circuits before the rng draw); explicit triggers above bypass this.
    if cfg.sample_rate > 0.0
        && !path_excluded(req.uri, cfg)
        && rng.random::<f64>() < cfg.sample_rate
    {
        return Some(ActivationDecision {
            source: ActivationSource::SampleRate,
            mode: ProfilingMode::ProfileAll,
            run_id: generate_run_id(req, rng),
        });
    }

    None
}

fn check_explicit(req: &PluginRequestView, cfg: &ProfilerConfig) -> Option<ActivationSource> {
    if let Some(v) = req.header("x-oxphp-profile").and_then(|h| h.to_str().ok()) {
        if validate_token(v, cfg) {
            return Some(ActivationSource::Header);
        }
    }
    if let Some(v) = req.cookie("OXPROF") {
        if validate_token(v, cfg) {
            return Some(ActivationSource::Cookie);
        }
    }
    if let Some(v) = extract_query_param(req.uri, "__oxprof") {
        if validate_token(&v, cfg) {
            return Some(ActivationSource::Query);
        }
    }
    None
}

fn validate_token(provided: &str, cfg: &ProfilerConfig) -> bool {
    match &cfg.auth_token {
        None => !provided.is_empty(),
        Some(expected) => {
            // Constant-time compare. Different lengths are rejected without byte-comparing.
            let a = provided.as_bytes();
            let b = expected.as_bytes();
            if a.len() != b.len() {
                return false;
            }
            a.ct_eq(b).into()
        }
    }
}

fn extract_query_param(uri: &http::Uri, key: &str) -> Option<String> {
    uri.query().and_then(|q| {
        q.split('&').find_map(|kv| {
            let (k, v) = kv.split_once('=')?;
            if k == key {
                Some(v.to_string())
            } else {
                None
            }
        })
    })
}

/// True when the request path matches `PROFILER_EXCLUDE_PATHS`. The leading
/// `/` is stripped to match the normalized patterns. `None` set → never
/// excluded. Consulted by `sample_rate` activation only — explicit triggers
/// (header/cookie/query) bypass this and run first.
///
/// Matches the **raw** request path (`uri.path()`): unlike `PHP_DENY_PATHS`,
/// which runs against the routing layer's sanitized path, this runs at
/// RequestReceived — before routing — so no percent-decoding or `..`
/// normalization has happened yet. Only the glob *syntax* is shared with
/// `PHP_DENY_PATHS`, not the matched input. This is deliberate: exclusion only
/// suppresses sampling overhead (not a security boundary), and framework
/// toolbar paths (`/_wdt/...`, `/_profiler/...`) are literal ASCII that never
/// arrive percent-encoded or with dot-segments.
fn path_excluded(uri: &http::Uri, cfg: &ProfilerConfig) -> bool {
    match &cfg.exclude_paths {
        None => false,
        Some(set) => {
            let path = uri.path().strip_prefix('/').unwrap_or(uri.path());
            set.is_match(path)
        }
    }
}

fn generate_run_id<R: Rng + ?Sized>(req: &PluginRequestView, rng: &mut R) -> String {
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let req_id_prefix: String = req.request_id.chars().take(8).collect();
    let rand4: u16 = rng.random();
    format!("{ts_ms}-{req_id_prefix}-{rand4:04x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::cookies::PluginCookies;
    use http::{HeaderMap, Method, Uri};
    use rand::rngs::StdRng;
    use rand::SeedableRng;
    use std::net::SocketAddr;
    use std::sync::Arc;

    fn base_config(enabled: bool) -> ProfilerConfig {
        ProfilerConfig {
            enabled,
            ..ProfilerConfig::default()
        }
    }

    /// Hold the allocations that back a `PluginRequestView` for the duration
    /// of a single test. Required because `PluginRequestView` borrows its
    /// fields.
    struct ViewFixture {
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        cookies: PluginCookies,
        metadata: Vec<(String, String)>,
        request_id: String,
        addr: SocketAddr,
    }

    impl ViewFixture {
        fn new(uri: &str, headers: HeaderMap, cookies: Vec<(&'static str, &'static str)>) -> Self {
            Self {
                method: Method::GET,
                uri: uri.parse().unwrap(),
                headers,
                cookies: PluginCookies {
                    cookies: cookies
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                },
                metadata: Vec::new(),
                request_id: "req-1234abcdef".to_string(),
                addr: "127.0.0.1:0".parse().unwrap(),
            }
        }

        fn view(&self) -> PluginRequestView<'_> {
            PluginRequestView::new(
                &self.method,
                &self.uri,
                self.addr,
                &self.request_id,
                &self.headers,
                PluginCookies {
                    cookies: self.cookies.cookies.clone(),
                },
                &self.metadata,
            )
        }
    }

    #[test]
    fn test_disabled_returns_none() {
        let cfg = base_config(false);
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/", HeaderMap::new(), vec![]);
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_none());
    }

    #[test]
    fn test_header_activates_without_token() {
        let cfg = base_config(true); // auth_token = None
        let mut rng = StdRng::seed_from_u64(0);
        let mut h = HeaderMap::new();
        h.insert("x-oxphp-profile", "anything".parse().unwrap());
        let fx = ViewFixture::new("/", h, vec![]);
        let d = should_profile(&fx.view(), &cfg, &mut rng).expect("should activate");
        assert_eq!(d.source, ActivationSource::Header);
        assert_eq!(d.mode, ProfilingMode::ProfileAll);
        assert!(!d.run_id.is_empty());
    }

    #[test]
    fn test_cookie_activates_without_token() {
        let cfg = base_config(true);
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/", HeaderMap::new(), vec![("OXPROF", "whatever")]);
        let d = should_profile(&fx.view(), &cfg, &mut rng).unwrap();
        assert_eq!(d.source, ActivationSource::Cookie);
    }

    #[test]
    fn test_query_activates_without_token() {
        let cfg = base_config(true);
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/foo?__oxprof=yes&other=ok", HeaderMap::new(), vec![]);
        let d = should_profile(&fx.view(), &cfg, &mut rng).unwrap();
        assert_eq!(d.source, ActivationSource::Query);
    }

    #[test]
    fn test_header_priority_over_cookie_and_query() {
        let cfg = base_config(true);
        let mut rng = StdRng::seed_from_u64(0);
        let mut h = HeaderMap::new();
        h.insert("x-oxphp-profile", "x".parse().unwrap());
        let fx = ViewFixture::new("/?__oxprof=q", h, vec![("OXPROF", "c")]);
        assert_eq!(
            should_profile(&fx.view(), &cfg, &mut rng).unwrap().source,
            ActivationSource::Header
        );
    }

    #[test]
    fn test_cookie_priority_over_query() {
        let cfg = base_config(true);
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/?__oxprof=q", HeaderMap::new(), vec![("OXPROF", "c")]);
        assert_eq!(
            should_profile(&fx.view(), &cfg, &mut rng).unwrap().source,
            ActivationSource::Cookie
        );
    }

    #[test]
    fn test_token_required_and_correct() {
        let mut cfg = base_config(true);
        cfg.auth_token = Some(Arc::<str>::from("secret-123"));
        let mut rng = StdRng::seed_from_u64(0);
        let mut h = HeaderMap::new();
        h.insert("x-oxphp-profile", "secret-123".parse().unwrap());
        let fx = ViewFixture::new("/", h, vec![]);
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_some());
    }

    #[test]
    fn test_token_required_and_incorrect() {
        let mut cfg = base_config(true);
        cfg.auth_token = Some(Arc::<str>::from("secret-123"));
        let mut rng = StdRng::seed_from_u64(0);
        let mut h = HeaderMap::new();
        h.insert("x-oxphp-profile", "wrong-1234".parse().unwrap());
        let fx = ViewFixture::new("/", h, vec![]);
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_none());
    }

    #[test]
    fn test_token_required_length_mismatch_rejected() {
        let mut cfg = base_config(true);
        cfg.auth_token = Some(Arc::<str>::from("long-secret"));
        let mut rng = StdRng::seed_from_u64(0);
        let mut h = HeaderMap::new();
        h.insert("x-oxphp-profile", "short".parse().unwrap());
        let fx = ViewFixture::new("/", h, vec![]);
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_none());
    }

    #[test]
    fn test_sample_rate_activation() {
        let mut cfg = base_config(true);
        cfg.sample_rate = 1.0; // always fires
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/", HeaderMap::new(), vec![]);
        let d = should_profile(&fx.view(), &cfg, &mut rng).unwrap();
        assert_eq!(d.source, ActivationSource::SampleRate);
    }

    #[test]
    fn test_sample_rate_zero_means_off() {
        let mut cfg = base_config(true);
        cfg.sample_rate = 0.0;
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/", HeaderMap::new(), vec![]);
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_none());
    }

    fn config_excluding(patterns: &str) -> ProfilerConfig {
        use globset::{GlobBuilder, GlobSetBuilder};
        let mut b = GlobSetBuilder::new();
        for p in patterns.split(',') {
            let p = p.trim().strip_prefix('/').unwrap_or(p.trim());
            b.add(GlobBuilder::new(p).literal_separator(true).build().unwrap());
        }
        ProfilerConfig {
            enabled: true,
            sample_rate: 1.0, // always fires unless excluded
            exclude_paths: Some(std::sync::Arc::new(b.build().unwrap())),
            ..ProfilerConfig::default()
        }
    }

    #[test]
    fn test_excluded_path_not_sampled() {
        let cfg = config_excluding("/_profiler/**");
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/_profiler/abc", HeaderMap::new(), vec![]);
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_none());
    }

    #[test]
    fn test_non_excluded_path_is_sampled() {
        let cfg = config_excluding("/_profiler/**");
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/app/dashboard", HeaderMap::new(), vec![]);
        let d = should_profile(&fx.view(), &cfg, &mut rng).expect("sampled");
        assert_eq!(d.source, ActivationSource::SampleRate);
    }

    #[test]
    fn test_explicit_header_bypasses_exclusion() {
        let cfg = config_excluding("/_profiler/**");
        let mut rng = StdRng::seed_from_u64(0);
        let mut h = HeaderMap::new();
        h.insert("x-oxphp-profile", "anything".parse().unwrap());
        let fx = ViewFixture::new("/_profiler/abc", h, vec![]);
        let d = should_profile(&fx.view(), &cfg, &mut rng).expect("explicit wins");
        assert_eq!(d.source, ActivationSource::Header);
    }

    #[test]
    fn test_exclusion_matches_raw_path_not_decoded() {
        // Documented behavior: matching runs against the raw request path at
        // RequestReceived (before routing), so no percent-decoding happens.
        // An exact pattern keyed on the decoded form does not match the
        // encoded request, so it is still sampled. Pins the intentional
        // divergence from PHP_DENY_PATHS (which matches the sanitized path).
        // (A `**` subtree pattern would still match the encoded segment as a
        // literal — the gap only shows on exact patterns and on dot-segments.)
        let cfg = config_excluding("/_profiler/abc");
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/_profiler/%61bc", HeaderMap::new(), vec![]);
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_some());
    }

    #[test]
    fn test_bare_excluded_path_needs_its_own_pattern() {
        // "/_profiler/**" does NOT cover bare "/_profiler"; the recipe adds it.
        let only_subtree = config_excluding("/_profiler/**");
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/_profiler", HeaderMap::new(), vec![]);
        assert!(should_profile(&fx.view(), &only_subtree, &mut rng).is_some());

        let with_bare = config_excluding("/_profiler,/_profiler/**");
        let mut rng2 = StdRng::seed_from_u64(0);
        let fx2 = ViewFixture::new("/_profiler", HeaderMap::new(), vec![]);
        assert!(should_profile(&fx2.view(), &with_bare, &mut rng2).is_none());
    }

    #[test]
    fn test_run_id_shape() {
        let cfg = base_config(true);
        let mut rng = StdRng::seed_from_u64(1234);
        let mut h = HeaderMap::new();
        h.insert("x-oxphp-profile", "x".parse().unwrap());
        // Use a request_id with no hyphens so split('-') is unambiguous.
        let fx = ViewFixture {
            request_id: "req1234abcdef".to_string(),
            ..ViewFixture::new("/", h, vec![])
        };
        let d = should_profile(&fx.view(), &cfg, &mut rng).unwrap();
        // Format: <ts_ms>-<req_id[:8]>-<rand[:4 hex]>
        let parts: Vec<&str> = d.run_id.split('-').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1], "req1234a");
        assert_eq!(parts[2].len(), 4);
        // Random part is a hex u16.
        assert!(u16::from_str_radix(parts[2], 16).is_ok());
    }
}
