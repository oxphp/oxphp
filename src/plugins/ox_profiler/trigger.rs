//! Trigger logic — decide whether an incoming request should be profiled.
//!
//! `should_profile` is a pure function: it inspects a `PluginRequestView`
//! against the configured `ProfilerConfig` and returns
//! `Some(ActivationDecision)` if the request meets the activation criteria
//! (header token match, or random sampling hit).
//! `None` means "run at ApmOnly or Off as usual".

use rand::{Rng, RngExt};
use subtle::ConstantTimeEq;

use crate::plugin::handler::PluginRequestView;
use crate::profiling::ProfilingMode;

use super::config::ProfilerConfig;

/// Why profiling was activated for a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ActivationSource {
    // `index.json` survives an upgrade, and retention drops a line it cannot
    // parse, leaving that run's files with nothing to delete them. These two
    // names occur only in such lines.
    #[serde(alias = "Cookie", alias = "Query")]
    Header,
    SampleRate,
    /// No trigger matched and the request was profiled anyway. Reconstructed
    /// at completion from the absence of a decision, so it names every such
    /// run: PHP turning profiling on mid-request with `OxPHP\Profile\start()`,
    /// which is the usual one, and equally another plugin selecting
    /// `ProfileAll` through `set_profiling_mode` without going through this
    /// trigger. This is the one activation `should_profile` never returns.
    Sdk,
}

impl ActivationSource {
    /// Wire name, shared by the Prometheus label and the
    /// request-metadata entry that carries the decision from the request
    /// handler to the complete handler. Kept in step with the label table in
    /// `storage::metrics` by a test there.
    pub fn as_wire(self) -> &'static str {
        match self {
            ActivationSource::Header => "header",
            ActivationSource::SampleRate => "sample",
            ActivationSource::Sdk => "sdk",
        }
    }

    /// Inverse of [`as_wire`](Self::as_wire). `None` for anything else, which
    /// the complete handler treats as "no decision was recorded".
    pub fn from_wire(s: &str) -> Option<Self> {
        Some(match s {
            "header" => ActivationSource::Header,
            "sample" => ActivationSource::SampleRate,
            "sdk" => ActivationSource::Sdk,
            _ => return None,
        })
    }
}

/// The outcome of `should_profile` when activation succeeds.
#[derive(Debug, Clone)]
pub struct ActivationDecision {
    pub source: ActivationSource,
    pub mode: ProfilingMode,
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

    if header_token_matches(req, cfg) {
        return Some(ActivationDecision {
            source: ActivationSource::Header,
            mode: ProfilingMode::ProfileAll,
        });
    }

    // Random sampling: no token required. Excluded paths are never sampled
    // (short-circuits before the rng draw); the header trigger above bypasses this.
    if cfg.sample_rate > 0.0
        && !path_excluded(req.uri, cfg)
        && rng.random::<f64>() < cfg.sample_rate
    {
        return Some(ActivationDecision {
            source: ActivationSource::SampleRate,
            mode: ProfilingMode::ProfileAll,
        });
    }

    None
}

fn header_token_matches(req: &PluginRequestView, cfg: &ProfilerConfig) -> bool {
    req.header("x-oxphp-profile")
        .and_then(|h| h.to_str().ok())
        .is_some_and(|v| validate_token(v, cfg))
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

/// True when the request path matches `PROFILER_EXCLUDE_PATHS`. The leading
/// `/` is stripped to match the normalized patterns. `None` set → never
/// excluded. Consulted by `sample_rate` activation only — the header trigger
/// bypasses this and runs first.
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

/// Mint the identifier a stored run is filed under:
/// `<ts_ms>-<request_id[:8]>-<rand4>`.
///
/// Called once per stored run, from the complete handler, and never from the
/// trigger. The request id is only final after every `RequestReceived` handler
/// has run — under `OTEL_ENABLED=true`, `ox_otel` replaces it with one derived
/// from the trace context — so minting at admission would embed an id that is
/// not the one the run is filed under.
///
/// The id doubles as the stored file's name, so it has to survive
/// `run_id_is_safe`. It does for both ids the server produces on its own — the
/// twenty hex characters of a plain request id, and the `<trace_id>-<span_id>`
/// prefixes `ox_otel` substitutes, whose first eight are hex too — and for any
/// inbound override whose first eight characters are alphanumerics, `-` or
/// `_`. It does not when one of those eight is a
/// `.`, which the inbound `X-Request-ID` check admits and the disk writer
/// refuses. Only the *disk copy* is lost, and only there is anything logged:
/// by that point the run has already been counted in `runs_total`, put in the
/// in-memory cache under that same name — where `/__profiler/runs/{id}` will
/// not fetch it, because the route screens the id against this alphabet too
/// and answers `400` — and, on a separate branch, pushed to an external
/// collector, which does not screen it at all. Only the prefix is at stake:
/// a `.` past the eighth character no longer reaches the name, where naming
/// the file after the request id outright put the whole id at risk.
pub fn generate_run_id<R: Rng + ?Sized>(request_id: &str, rng: &mut R) -> String {
    let ts_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);

    let req_id_prefix: String = request_id.chars().take(8).collect();
    let rand4: u16 = rng.random();
    format!("{ts_ms}-{req_id_prefix}-{rand4:04x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin::cookies::extract_plugin_cookies;
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

    /// The prefix the plugin manager hands this plugin at init —
    /// `format!("__oxp_{}_", plugin.name())` in `plugin::manager` — for a
    /// plugin whose `name()` is `profiler`.
    const PROFILER_COOKIE_PREFIX: &str = "__oxp_profiler_";

    /// Hold the allocations that back a `PluginRequestView` for the duration
    /// of a single test. Required because `PluginRequestView` borrows its
    /// fields.
    ///
    /// The cookies arrive as a raw `Cookie` header and the plugin's namespaced
    /// view of them is derived from that header exactly as
    /// `PluginRequestWrapper` derives it, rather than being handed over
    /// already parsed. Building `PluginCookies` directly would let a trigger
    /// read a cookie name the real pipeline never produces and still pass
    /// here.
    struct ViewFixture {
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        metadata: Vec<crate::events::MetadataEntry>,
        request_id: String,
        addr: SocketAddr,
    }

    impl ViewFixture {
        /// `cookie_header` is the verbatim value of the `Cookie` request
        /// header, e.g. `"session=abc; theme=dark"`. Empty means the request
        /// carries no `Cookie` header at all.
        fn new(uri: &str, mut headers: HeaderMap, cookie_header: &str) -> Self {
            if !cookie_header.is_empty() {
                headers.insert(
                    http::header::COOKIE,
                    http::HeaderValue::from_str(cookie_header).unwrap(),
                );
            }
            Self {
                method: Method::GET,
                uri: uri.parse().unwrap(),
                headers,
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
                extract_plugin_cookies(&self.headers, PROFILER_COOKIE_PREFIX),
                &self.metadata,
            )
        }
    }

    #[test]
    fn test_disabled_returns_none() {
        let cfg = base_config(false);
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/", HeaderMap::new(), "");
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_none());
    }

    #[test]
    fn test_header_activates_without_token() {
        let cfg = base_config(true); // auth_token = None
        let mut rng = StdRng::seed_from_u64(0);
        let mut h = HeaderMap::new();
        h.insert("x-oxphp-profile", "anything".parse().unwrap());
        let fx = ViewFixture::new("/", h, "");
        let d = should_profile(&fx.view(), &cfg, &mut rng).expect("should activate");
        assert_eq!(d.source, ActivationSource::Header);
        assert_eq!(d.mode, ProfilingMode::ProfileAll);
    }

    #[test]
    fn cookie_and_query_do_not_activate() {
        let mut cfg = base_config(true);
        cfg.auth_token = Some(Arc::<str>::from("secret-123"));
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new(
            "/?__oxprof=secret-123",
            HeaderMap::new(),
            "OXPROF=secret-123",
        );
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_none());
    }

    #[test]
    fn test_token_required_and_correct() {
        let mut cfg = base_config(true);
        cfg.auth_token = Some(Arc::<str>::from("secret-123"));
        let mut rng = StdRng::seed_from_u64(0);
        let mut h = HeaderMap::new();
        h.insert("x-oxphp-profile", "secret-123".parse().unwrap());
        let fx = ViewFixture::new("/", h, "");
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_some());
    }

    #[test]
    fn test_token_required_and_incorrect() {
        let mut cfg = base_config(true);
        cfg.auth_token = Some(Arc::<str>::from("secret-123"));
        let mut rng = StdRng::seed_from_u64(0);
        let mut h = HeaderMap::new();
        h.insert("x-oxphp-profile", "wrong-1234".parse().unwrap());
        let fx = ViewFixture::new("/", h, "");
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_none());
    }

    #[test]
    fn test_token_required_length_mismatch_rejected() {
        let mut cfg = base_config(true);
        cfg.auth_token = Some(Arc::<str>::from("long-secret"));
        let mut rng = StdRng::seed_from_u64(0);
        let mut h = HeaderMap::new();
        h.insert("x-oxphp-profile", "short".parse().unwrap());
        let fx = ViewFixture::new("/", h, "");
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_none());
    }

    #[test]
    fn test_sample_rate_activation() {
        let mut cfg = base_config(true);
        cfg.sample_rate = 1.0; // always fires
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/", HeaderMap::new(), "");
        let d = should_profile(&fx.view(), &cfg, &mut rng).unwrap();
        assert_eq!(d.source, ActivationSource::SampleRate);
    }

    /// `PROFILER_AUTH_TOKEN` gates the header trigger, not
    /// `PROFILER_SAMPLE_RATE` — a request carrying no trigger at all is still
    /// sampled while a token is configured. Pins the documented contract; it
    /// holds today, and this is what would break if the token check ever moved
    /// ahead of the sampling draw.
    #[test]
    fn test_sample_rate_ignores_a_configured_token() {
        let mut cfg = base_config(true);
        cfg.auth_token = Some(Arc::<str>::from("secret-123"));
        cfg.sample_rate = 1.0;
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/", HeaderMap::new(), "");
        let d = should_profile(&fx.view(), &cfg, &mut rng).unwrap();
        assert_eq!(d.source, ActivationSource::SampleRate);
    }

    #[test]
    fn test_sample_rate_zero_means_off() {
        let mut cfg = base_config(true);
        cfg.sample_rate = 0.0;
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/", HeaderMap::new(), "");
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
        let fx = ViewFixture::new("/_profiler/abc", HeaderMap::new(), "");
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_none());
    }

    #[test]
    fn test_non_excluded_path_is_sampled() {
        let cfg = config_excluding("/_profiler/**");
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/app/dashboard", HeaderMap::new(), "");
        let d = should_profile(&fx.view(), &cfg, &mut rng).expect("sampled");
        assert_eq!(d.source, ActivationSource::SampleRate);
    }

    #[test]
    fn test_explicit_header_bypasses_exclusion() {
        let cfg = config_excluding("/_profiler/**");
        let mut rng = StdRng::seed_from_u64(0);
        let mut h = HeaderMap::new();
        h.insert("x-oxphp-profile", "anything".parse().unwrap());
        let fx = ViewFixture::new("/_profiler/abc", h, "");
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
        let fx = ViewFixture::new("/_profiler/%61bc", HeaderMap::new(), "");
        assert!(should_profile(&fx.view(), &cfg, &mut rng).is_some());
    }

    #[test]
    fn test_bare_excluded_path_needs_its_own_pattern() {
        // "/_profiler/**" does NOT cover bare "/_profiler"; the recipe adds it.
        let only_subtree = config_excluding("/_profiler/**");
        let mut rng = StdRng::seed_from_u64(0);
        let fx = ViewFixture::new("/_profiler", HeaderMap::new(), "");
        assert!(should_profile(&fx.view(), &only_subtree, &mut rng).is_some());

        let with_bare = config_excluding("/_profiler,/_profiler/**");
        let mut rng2 = StdRng::seed_from_u64(0);
        let fx2 = ViewFixture::new("/_profiler", HeaderMap::new(), "");
        assert!(should_profile(&fx2.view(), &with_bare, &mut rng2).is_none());
    }

    #[test]
    fn test_source_wire_names_round_trip() {
        // The wire name is what crosses the request-metadata channel between
        // the request handler and the complete handler; a variant whose name
        // does not come back is a run filed under the wrong source.
        for src in [
            ActivationSource::Header,
            ActivationSource::SampleRate,
            ActivationSource::Sdk,
        ] {
            assert_eq!(
                ActivationSource::from_wire(src.as_wire()),
                Some(src),
                "{src:?} does not survive its own wire name"
            );
        }
    }

    #[test]
    fn test_unknown_wire_name_is_not_a_source() {
        assert_eq!(ActivationSource::from_wire(""), None);
        assert_eq!(ActivationSource::from_wire("Header"), None);
        assert_eq!(ActivationSource::from_wire("sampleRate"), None);
    }

    #[test]
    fn from_wire_knows_three_sources() {
        for (wire, expected) in [
            ("header", Some(ActivationSource::Header)),
            ("sample", Some(ActivationSource::SampleRate)),
            ("sdk", Some(ActivationSource::Sdk)),
            ("cookie", None),
            ("query", None),
        ] {
            assert_eq!(ActivationSource::from_wire(wire), expected, "{wire}");
        }
    }

    #[test]
    fn test_generated_run_id_is_path_safe() {
        // The id names the file a run is stored in, so the disk writer's
        // validator has to accept it — for the id the server mints and for an
        // override a plugin or an inbound header supplied.
        let mut rng = StdRng::seed_from_u64(7);
        for request_id in ["6aad6572c21c00000014", "req-test", "", "a"] {
            let run_id = generate_run_id(request_id, &mut rng);
            assert!(
                crate::plugins::ox_profiler::storage::disk::run_id_is_safe(&run_id),
                "run_id {run_id} from request_id {request_id:?} is not storable"
            );
        }
    }

    #[test]
    fn test_a_dot_in_the_first_eight_characters_yields_an_unstorable_run_id() {
        // `X-Request-ID: v1.2.3` passes the inbound check, which allows `.`,
        // and the disk writer's alphabet does not. Only the prefix reaches the
        // name, so a dot past the eighth character is harmless — both halves
        // pinned here so neither is mistaken for the other.
        let mut rng = StdRng::seed_from_u64(7);
        let safe = crate::plugins::ox_profiler::storage::disk::run_id_is_safe;

        let in_prefix = generate_run_id("v1.2.3", &mut rng);
        assert!(!safe(&in_prefix), "{in_prefix} should not be storable");

        let past_prefix = generate_run_id("deadbeef.v2", &mut rng);
        assert!(safe(&past_prefix), "{past_prefix} should be storable");
    }

    #[test]
    fn test_run_id_shape() {
        let mut rng = StdRng::seed_from_u64(1234);
        // Use a request_id with no hyphens so split('-') is unambiguous.
        let run_id = generate_run_id("req1234abcdef", &mut rng);
        // Format: <ts_ms>-<req_id[:8]>-<rand[:4 hex]>
        let parts: Vec<&str> = run_id.split('-').collect();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[1], "req1234a");
        assert_eq!(parts[2].len(), 4);
        // Random part is a hex u16.
        assert!(u16::from_str_radix(parts[2], 16).is_ok());
    }
}
