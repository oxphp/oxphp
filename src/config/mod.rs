mod env_bool;
mod php_deny;
mod proxy;
mod server;
pub(crate) mod symlink_allow;
mod workers;

use std::fmt;
use std::path::{Path, PathBuf};

#[allow(unused_imports)] // consumed by feature-gated plugins
pub(crate) use env_bool::parse_bool_opt;
pub(crate) use env_bool::{parse_bool_strict, parse_env_bool};
pub use php_deny::{DeniedMeta, DenyFallback, PhpDeny, RoutingModeKind};
pub use proxy::{
    classify_bind_exposure, parse_cidr_list, BindExposure, IpAllowList, TrustedProxyConfig,
};
pub use server::{H2Config, ServerConfig};
pub use symlink_allow::SymlinkAllowList;
pub use workers::{parse_php_workers, WorkerMode};

/// Access log verbosity level.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum AccessLogLevel {
    /// Logging disabled.
    #[default]
    Off,
    /// Only log error responses (status >= 400).
    Error,
    /// Log every request.
    All,
}

impl fmt::Display for AccessLogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Off => f.write_str("off"),
            Self::Error => f.write_str("error"),
            Self::All => f.write_str("all"),
        }
    }
}

/// Top-level application configuration.
#[derive(Debug)]
pub struct Config {
    pub server: ServerConfig,
    pub log_level: String,
    pub executor_type: String,
    pub max_connections: usize,
    pub drain_timeout_seconds: u64,
    pub internal_addr: Option<String>,
    pub internal_allow_ips: Option<IpAllowList>,
    pub rate_limit: u32,
    pub rate_window_seconds: u64,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub error_pages_dir: Option<String>,
    pub compression_level: i32,
    pub access_log: AccessLogLevel,
    pub max_query_body: usize,
    /// Canonical entry script. `None` = direct file mapping (legacy traditional).
    /// `Some(*.php)` with `worker_mode_enabled=false` = front controller.
    /// `Some(non-.php)` with `worker_mode_enabled=false` = static fallback (SPA).
    /// `Some(*.php)` with `worker_mode_enabled=true` = worker bootstrap.
    pub entry_file: Option<PathBuf>,
    /// Explicit worker-mode toggle. When true, `entry_file` must be `Some(*.php)`.
    pub worker_mode_enabled: bool,
    /// Max memory (MB) before recycling a worker (0 = unlimited).
    pub worker_max_memory_mib: u64,
    /// Static file `Cache-Control: max-age` value, in seconds.
    /// `None` = no `Cache-Control` header sent.
    pub static_max_age: Option<u64>,
    /// Whether the in-memory file cache performs `stat()` revalidation on hit.
    /// `true` = check mtime each time and evict stale entries (development).
    /// `false` (default) = trust cache contents without revalidation (production).
    pub static_revalidate: bool,
    /// Number of dedicated async worker threads. 0 = async pool disabled.
    pub async_workers: usize,
    /// Bounded channel capacity for pending async tasks. 0 = auto (async_workers * 64).
    pub async_queue_capacity: usize,
    /// Per-worker bound on in-flight (queued + running) async tasks. The
    /// process-global cap is this value × async_workers; dispatches past it are
    /// rejected (non-blocking) with AsyncException. Default 256.
    pub async_max_fibers: usize,
    /// W3C Trace Context propagation enabled.
    pub trace_context: bool,
    /// Whether PHP superglobals ($_GET, $_POST, etc.) are populated.
    /// When false, only the object API (oxphp_http_request()) provides request data.
    pub superglobals_enabled: bool,
    /// PHP worker pool mode (static count or dynamic min:max).
    pub worker_mode: WorkerMode,
    /// True if `worker_mode` was auto-derived (env var unset or empty).
    pub worker_mode_auto: bool,
    /// Idle timeout for dynamic worker scale-down (seconds).
    pub worker_idle_timeout_seconds: u64,
    /// Effective number of Tokio runtime threads.
    pub tokio_workers: usize,
    /// Bounded channel capacity for PHP request queue.
    pub queue_capacity: usize,
    /// Trusted reverse proxy networks (CIDR). When set, X-Forwarded-* and
    /// Forwarded headers from these peers are trusted for client IP extraction.
    pub trusted_proxies: Option<TrustedProxyConfig>,
    /// HTTP/2 protocol tuning (stream limits, flow control, keep-alive).
    pub h2: H2Config,
}

/// Parse a duration string like `"30s"`, `"5m"`, `"2h"`, `"30d"`, `"1w"`,
/// `"1y"`, `"3600"`, or `"off"`. Bare numbers are treated as seconds.
///
/// Returns `Ok(None)` for the literal `"off"` (caller chooses whether that
/// disables a header / TTL / etc.), `Ok(Some(n))` for a valid duration, and
/// `Err(_)` for empty input or an unrecognised value. Callers should filter
/// empty values at the env layer (`FOO=`) before calling, mirroring the bool
/// parser's policy.
pub fn parse_duration(s: &str) -> Result<Option<u64>, String> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("off") {
        return Ok(None);
    }
    if s.is_empty() {
        return Err("expected a duration like 30s, 5m, 2h, 30d, 1w, 1y, a bare number of seconds, or off — got empty string".to_string());
    }
    // Bare number = seconds
    if let Ok(secs) = s.parse::<u64>() {
        return Ok(Some(secs));
    }
    let (num_str, suffix) = s.split_at(s.len() - 1);
    let num: u64 = num_str.parse().map_err(|_| {
        format!(
            "expected a duration like 30s, 5m, 2h, 30d, 1w, 1y, a bare number of seconds, or off — got {s:?}"
        )
    })?;
    let multiplier = match suffix {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        "y" => 31_536_000,
        _ => {
            return Err(format!(
                "unknown duration suffix {suffix:?} in {s:?} — expected s/m/h/d/w/y"
            ))
        }
    };
    Ok(Some(num.saturating_mul(multiplier)))
}

/// Compile a comma-separated glob list into a `GlobSet` plus the normalized
/// source patterns.
///
/// Each pattern is trimmed, empties are dropped, and a leading `/` is stripped
/// so patterns match sanitized URI paths (whose leading `/` is also removed).
/// Globs are built with `literal_separator(true)`: `*` does not cross `/`,
/// `**` does. Returns `Ok(None)` when the list is empty after trimming.
/// `label` names the source (e.g. an env var) for error messages.
///
/// Shared by `PHP_DENY_PATHS` and `PROFILER_EXCLUDE_PATHS` so their glob
/// semantics cannot drift apart.
pub(crate) fn compile_glob_csv(
    raw: &str,
    label: &str,
) -> Result<Option<(globset::GlobSet, Vec<String>)>, String> {
    let patterns: Vec<String> = raw
        .split(',')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| p.strip_prefix('/').unwrap_or(p).to_string())
        .collect();
    if patterns.is_empty() {
        return Ok(None);
    }
    let mut builder = globset::GlobSetBuilder::new();
    for p in &patterns {
        let glob = globset::GlobBuilder::new(p)
            .literal_separator(true)
            .build()
            .map_err(|e| format!("{label} pattern {p:?}: {e}"))?;
        builder.add(glob);
    }
    let set = builder.build().map_err(|e| format!("{label} build: {e}"))?;
    Ok(Some((set, patterns)))
}

/// Normalize `INTERNAL_ADDR`. A port-only form (`:9090`) binds loopback by
/// default so the internal endpoints are not exposed off-host unless the
/// operator opts in with an explicit address such as `0.0.0.0:9090`.
fn normalize_internal_addr(addr: &str) -> String {
    let trimmed = addr.trim();
    if let Some(port) = trimmed.strip_prefix(':') {
        if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) {
            return format!("127.0.0.1:{port}");
        }
    }
    trimmed.to_string()
}

/// Resolve `static_max_age` from new and legacy env values.
/// `new` is `STATIC_MAX_AGE`, `legacy` is the deprecated `STATIC_CACHE_TTL`.
/// Both are expected to be non-empty (callers strip empty/unset upstream).
/// New value wins. When both are unset, defaults to 30 days. Garbage values
/// surface as a startup error tagged with the variable that supplied them.
fn resolve_static_max_age(
    new: Option<&str>,
    legacy: Option<&str>,
) -> Result<Option<u64>, crate::types::BoxError> {
    if let Some(val) = new {
        return parse_duration(val)
            .map_err(|e| -> crate::types::BoxError { format!("STATIC_MAX_AGE: {e}").into() });
    }
    if let Some(val) = legacy {
        return parse_duration(val)
            .map_err(|e| -> crate::types::BoxError { format!("STATIC_CACHE_TTL: {e}").into() });
    }
    Ok(Some(2_592_000))
}

/// Resolve `static_revalidate` from new and legacy env values.
/// `new` is `STATIC_REVALIDATE` (strictly parsed boolean — invalid values are
/// rejected with a startup error). `legacy` is the deprecated `STATIC_CACHE`
/// where the value `off` historically meant "enable mtime revalidation"
/// (kept lenient on purpose, since it is a compatibility shim). New value
/// wins; default is `false`.
fn resolve_static_revalidate(
    new: Option<&str>,
    legacy: Option<&str>,
) -> Result<bool, crate::types::BoxError> {
    if let Some(val) = new {
        return parse_bool_strict(val)
            .map_err(|e| -> crate::types::BoxError { format!("STATIC_REVALIDATE: {e}").into() });
    }
    match legacy {
        Some(val) => Ok(val.eq_ignore_ascii_case("off")),
        None => Ok(false),
    }
}

impl Config {
    pub fn from_env() -> Result<Self, crate::types::BoxError> {
        let server = ServerConfig::from_env()?;
        let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let executor_type = std::env::var("EXECUTOR")
            .unwrap_or_else(|_| "sapi".to_string())
            .to_ascii_lowercase();
        let max_connections = std::env::var("MAX_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(10_000);
        let drain_timeout_seconds = std::env::var("DRAIN_TIMEOUT_SECONDS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(30);
        let internal_addr = std::env::var("INTERNAL_ADDR")
            .ok()
            .map(|a| normalize_internal_addr(&a));
        let rate_limit = std::env::var("RATE_LIMIT")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let rate_window_seconds = std::env::var("RATE_WINDOW_SECONDS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);
        let tls_cert = std::env::var("TLS_CERT").ok();
        let tls_key = std::env::var("TLS_KEY").ok();
        let error_pages_dir = std::env::var("ERROR_PAGES_DIR").ok();
        let compression_level: i32 = match std::env::var("COMPRESSION_LEVEL") {
            Ok(val) => match val.parse::<i32>() {
                Ok(v) if (0..=11).contains(&v) => v,
                _ => {
                    return Err(format!(
                        "COMPRESSION_LEVEL must be 0-11 (got {val:?}), 0 = disabled"
                    )
                    .into());
                }
            },
            Err(_) => 4,
        };
        let access_log = match std::env::var("ACCESS_LOG").as_deref() {
            Ok("all") => AccessLogLevel::All,
            Ok("error") => AccessLogLevel::Error,
            Ok("") | Err(_) => AccessLogLevel::Off,
            Ok(other) => {
                tracing::warn!(
                    value = %other,
                    "unknown ACCESS_LOG value, expected \"all\", \"error\", or empty — defaulting to off"
                );
                AccessLogLevel::Off
            }
        };
        let max_query_body = std::env::var("MAX_QUERY_BODY")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(512 * 1024);

        let (entry_file, worker_mode_enabled) = resolve_entry_and_mode(&server.document_root)?;

        // Parsed-but-ignored deprecation. Drop in a future release once telemetry
        // shows zero downstream usage.
        if std::env::var_os("WORKER_MAX_REQUESTS").is_some() {
            tracing::warn!(
                "WORKER_MAX_REQUESTS is deprecated and ignored — \
                 use WORKER_MAX_MEMORY_MIB or Worker::scheduleExit()"
            );
        }
        let worker_max_memory_mib = std::env::var("WORKER_MAX_MEMORY_MIB")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        // Empty values (`FOO=`) collapse to "unset" — same policy as the
        // bool parser, so Docker Compose substitutions like `FOO=${FOO}`
        // with a missing host var fall back to defaults instead of erroring.
        let strip_empty = |s: String| (!s.trim().is_empty()).then_some(s);
        let new_max_age = std::env::var("STATIC_MAX_AGE").ok().and_then(strip_empty);
        let new_revalidate = std::env::var("STATIC_REVALIDATE")
            .ok()
            .and_then(strip_empty);
        let legacy_ttl = std::env::var("STATIC_CACHE_TTL").ok().and_then(strip_empty);
        let legacy_cache = std::env::var("STATIC_CACHE").ok().and_then(strip_empty);

        if legacy_ttl.is_some() {
            tracing::warn!("STATIC_CACHE_TTL is deprecated, use STATIC_MAX_AGE instead");
            if new_max_age.is_some() {
                tracing::warn!(
                    "both STATIC_CACHE_TTL and STATIC_MAX_AGE set; STATIC_MAX_AGE takes precedence"
                );
            }
        }
        if legacy_cache.is_some() {
            tracing::warn!("STATIC_CACHE is deprecated, use STATIC_REVALIDATE instead");
            if new_revalidate.is_some() {
                tracing::warn!(
                    "both STATIC_CACHE and STATIC_REVALIDATE set; STATIC_REVALIDATE takes precedence"
                );
            }
        }

        let static_max_age = resolve_static_max_age(new_max_age.as_deref(), legacy_ttl.as_deref())?;
        let static_revalidate =
            resolve_static_revalidate(new_revalidate.as_deref(), legacy_cache.as_deref())?;

        let async_workers: usize = std::env::var("ASYNC_WORKERS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let async_queue_capacity: usize = std::env::var("ASYNC_QUEUE_CAPACITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let async_max_fibers: usize = std::env::var("ASYNC_MAX_FIBERS")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&n| n > 0)
            .unwrap_or(256);

        let trace_context = parse_env_bool("TRACE_CONTEXT", false)?;
        let superglobals_enabled = parse_env_bool("SUPERGLOBALS_ENABLED", true)?;

        let cpu = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let default_workers = (cpu / 2).max(1);

        let (worker_mode, worker_mode_auto) = match std::env::var("PHP_WORKERS") {
            Ok(val) if !val.is_empty() => {
                let mode =
                    parse_php_workers(&val).map_err(|e| -> crate::types::BoxError { e.into() })?;
                (mode, false)
            }
            _ => (WorkerMode::Static(default_workers), true),
        };

        let worker_idle_timeout_seconds: u64 = std::env::var("PHP_WORKERS_IDLE_SECONDS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(30);

        let tokio_workers = std::env::var("TOKIO_WORKERS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(default_workers);

        let queue_capacity = std::env::var("QUEUE_CAPACITY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(worker_mode.worker_count() * 128);

        let trusted_proxies = TrustedProxyConfig::from_env()
            .map_err(|e| -> crate::types::BoxError { format!("TRUSTED_PROXIES: {e}").into() })?;

        let internal_allow_ips = IpAllowList::from_env()
            .map_err(|e| -> crate::types::BoxError { format!("INTERNAL_ALLOW_IPS: {e}").into() })?;

        let h2 = H2Config::from_env(worker_mode.max_worker_count());

        Ok(Self {
            server,
            log_level,
            executor_type,
            max_connections,
            drain_timeout_seconds,
            internal_addr,
            internal_allow_ips,
            rate_limit,
            rate_window_seconds,
            tls_cert,
            tls_key,
            error_pages_dir,
            compression_level,
            access_log,
            max_query_body,
            entry_file,
            worker_mode_enabled,
            worker_max_memory_mib,
            static_max_age,
            static_revalidate,
            async_workers,
            async_queue_capacity,
            async_max_fibers,
            trace_context,
            superglobals_enabled,
            worker_mode,
            worker_mode_auto,
            worker_idle_timeout_seconds,
            tokio_workers,
            queue_capacity,
            trusted_proxies,
            h2,
        })
    }

    /// Build a minimal `Config` for unit tests without touching the
    /// process environment. All env-derived values are set to fixed
    /// defaults so tests are independent of `std::env` state.
    #[cfg(test)]
    pub(crate) fn test_minimal() -> Self {
        use std::time::Duration;
        Self {
            server: ServerConfig {
                listen_addr: "127.0.0.1:0".to_string(),
                document_root: PathBuf::from("/var/www/html/public"),
                header_read_timeout: Duration::from_secs(5),
            },
            log_level: "info".to_string(),
            executor_type: "stub".to_string(),
            max_connections: 10_000,
            drain_timeout_seconds: 30,
            internal_addr: None,
            internal_allow_ips: None,
            rate_limit: 0,
            rate_window_seconds: 60,
            tls_cert: None,
            tls_key: None,
            error_pages_dir: None,
            compression_level: 4,
            access_log: AccessLogLevel::Off,
            max_query_body: 512 * 1024,
            entry_file: None,
            worker_mode_enabled: false,
            worker_max_memory_mib: 0,
            static_max_age: Some(2_592_000),
            static_revalidate: false,
            async_workers: 0,
            async_queue_capacity: 0,
            async_max_fibers: 256,
            trace_context: false,
            superglobals_enabled: true,
            worker_mode: WorkerMode::Static(1),
            worker_mode_auto: false,
            worker_idle_timeout_seconds: 30,
            tokio_workers: 1,
            queue_capacity: 128,
            trusted_proxies: None,
            h2: H2Config::default(),
        }
    }

    /// Human-readable description of the worker pool (e.g. "4", "2:8", "4 (auto)").
    pub fn php_workers_display(&self) -> String {
        if self.worker_mode_auto {
            format!("{} (auto)", self.worker_mode)
        } else {
            self.worker_mode.to_string()
        }
    }

    /// Serialize configuration to JSON for the `/config` internal endpoint.
    /// Sensitive values (TLS paths) are redacted.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "listen_addr": self.server.listen_addr,
            "document_root": self.server.document_root.display().to_string(),
            "entry_file": self.entry_file.as_ref().map(|p| p.display().to_string()),
            "log_level": self.log_level,
            "executor_type": self.executor_type,
            "php_workers": self.php_workers_display(),
            "tokio_workers": self.tokio_workers,
            "queue_capacity": self.queue_capacity,
            "max_connections": self.max_connections,
            "drain_timeout_seconds": self.drain_timeout_seconds,
            "internal_addr": self.internal_addr,
            "header_timeout_seconds": self.server.header_read_timeout.as_secs(),
            "rate_limit": self.rate_limit,
            "rate_window_seconds": self.rate_window_seconds,
            "tls_enabled": self.tls_cert.is_some() && self.tls_key.is_some(),
            "error_pages_dir": self.error_pages_dir,
            "compression_level": self.compression_level,
            "access_log": self.access_log.to_string(),
            "max_query_body": self.max_query_body,
            "worker_mode_enabled": self.worker_mode_enabled,
            "worker_max_memory_mib": self.worker_max_memory_mib,
            "static_max_age": self.static_max_age,
            "static_revalidate": self.static_revalidate,
            "async_workers": self.async_workers,
            "async_queue_capacity": if self.async_queue_capacity > 0 {
                self.async_queue_capacity
            } else {
                self.async_workers * 64
            },
            "async_max_fibers": self.async_max_fibers,
            "async_in_flight_cap": self.async_max_fibers * self.async_workers,
            "trace_context": self.trace_context,
            "superglobals_enabled": self.superglobals_enabled,
            "trusted_proxies": self.trusted_proxies.is_some(),
        })
    }

    /// Validate the current configuration against the filesystem and the
    /// `WORKER_MODE_ENABLED`/`ENTRY_FILE` invariants.
    ///
    /// Returns a list of problems (empty = OK). Path checks cover
    /// `DOCUMENT_ROOT`, `ENTRY_FILE`, `TLS_CERT`, `TLS_KEY`, `ERROR_PAGES_DIR`.
    /// Worker-mode invariants: an entry file is required, and it must be `.php`.
    /// All problems are collected — the function never short-circuits.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        check_dir("DOCUMENT_ROOT", &self.server.document_root, &mut errors);
        if let Some(entry) = &self.entry_file {
            check_file("ENTRY_FILE", entry, &mut errors);
        }
        if self.worker_mode_enabled {
            match &self.entry_file {
                None => errors
                    .push("WORKER_MODE_ENABLED=true requires ENTRY_FILE to be set".to_string()),
                Some(entry) => {
                    let is_php = entry
                        .extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("php"));
                    if !is_php {
                        errors.push(format!(
                            "WORKER_MODE_ENABLED=true requires a .php ENTRY_FILE (got {})",
                            entry.display()
                        ));
                    }
                }
            }
        }
        if let Some(cert) = &self.tls_cert {
            check_file("TLS_CERT", Path::new(cert), &mut errors);
        }
        if let Some(key) = &self.tls_key {
            check_file("TLS_KEY", Path::new(key), &mut errors);
        }
        if let Some(dir) = &self.error_pages_dir {
            check_dir("ERROR_PAGES_DIR", Path::new(dir), &mut errors);
        }
        errors
    }
}

/// Resolve `ENTRY_FILE` and `WORKER_MODE_ENABLED` from the environment, taking
/// the deprecated `INDEX_FILE` / `WORKER_FILE` variables into account.
///
/// Precedence: `ENTRY_FILE` > `WORKER_FILE` > `INDEX_FILE`. Worker mode is
/// enabled when `WORKER_MODE_ENABLED=true` *or* when the legacy `WORKER_FILE`
/// is set. Old vars trigger `tracing::warn!` lines pointing at the new names.
fn resolve_entry_and_mode(
    document_root: &Path,
) -> Result<(Option<PathBuf>, bool), crate::types::BoxError> {
    let entry_file_env = std::env::var("ENTRY_FILE").ok().filter(|s| !s.is_empty());
    let worker_mode_explicit = parse_env_bool("WORKER_MODE_ENABLED", false)?;

    let legacy_worker_file = std::env::var("WORKER_FILE").ok().filter(|s| !s.is_empty());
    let legacy_index_file = std::env::var("INDEX_FILE").ok().filter(|s| !s.is_empty());

    if let Some(ref wf) = legacy_worker_file {
        tracing::warn!(
            "WORKER_FILE is deprecated — use WORKER_MODE_ENABLED=true and ENTRY_FILE={wf}"
        );
    }
    if let Some(ref idx) = legacy_index_file {
        tracing::warn!("INDEX_FILE is deprecated — use ENTRY_FILE={idx}");
    }

    let raw = entry_file_env
        .or_else(|| legacy_worker_file.clone())
        .or(legacy_index_file);

    let entry_file = match raw {
        Some(value) => Some(resolve_entry_file(&value, document_root)?),
        None => None,
    };

    let worker_mode_enabled = worker_mode_explicit || legacy_worker_file.is_some();

    Ok((entry_file, worker_mode_enabled))
}

/// Resolve a raw `ENTRY_FILE` value into an absolute, existence-checked path.
///
/// Relative paths (including `..`-relative) resolve against `document_root`.
/// The result is canonicalised so symlinks collapse and the executor sees a
/// stable path regardless of how `DOCUMENT_ROOT` was spelled.
fn resolve_entry_file(
    value: &str,
    document_root: &Path,
) -> Result<PathBuf, crate::types::BoxError> {
    let path = Path::new(value);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        document_root.join(path)
    };
    candidate
        .canonicalize()
        .map_err(|e| -> crate::types::BoxError {
            format!("ENTRY_FILE={value} cannot be resolved: {e}").into()
        })
}

fn check_dir(label: &str, path: &Path, errors: &mut Vec<String>) {
    match path.metadata() {
        Ok(m) if m.is_dir() => {}
        Ok(_) => errors.push(format!(
            "{label}: {} exists but is not a directory",
            path.display()
        )),
        Err(e) => errors.push(format!("{label}: {} — {e}", path.display())),
    }
}

fn check_file(label: &str, path: &Path, errors: &mut Vec<String>) {
    match path.metadata() {
        Ok(m) if m.is_file() => {}
        Ok(_) => errors.push(format!(
            "{label}: {} exists but is not a regular file",
            path.display()
        )),
        Err(e) => errors.push(format!("{label}: {} — {e}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_internal_addr_port_only_binds_loopback() {
        assert_eq!(normalize_internal_addr(":9090"), "127.0.0.1:9090");
        assert_eq!(normalize_internal_addr(" :9090 "), "127.0.0.1:9090");
    }

    #[test]
    fn test_normalize_internal_addr_explicit_unchanged() {
        assert_eq!(normalize_internal_addr("0.0.0.0:9090"), "0.0.0.0:9090");
        assert_eq!(normalize_internal_addr("127.0.0.1:9090"), "127.0.0.1:9090");
        assert_eq!(normalize_internal_addr("10.0.0.5:9090"), "10.0.0.5:9090");
    }

    #[test]
    fn test_normalize_internal_addr_non_numeric_port_unchanged() {
        assert_eq!(normalize_internal_addr(":abc"), ":abc");
    }

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("30s"), Ok(Some(30)));
        assert_eq!(parse_duration("0s"), Ok(Some(0)));
        assert_eq!(parse_duration("1s"), Ok(Some(1)));
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("5m"), Ok(Some(300)));
        assert_eq!(parse_duration("1m"), Ok(Some(60)));
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("1h"), Ok(Some(3600)));
        assert_eq!(parse_duration("24h"), Ok(Some(86400)));
    }

    #[test]
    fn test_parse_duration_days() {
        assert_eq!(parse_duration("30d"), Ok(Some(2_592_000)));
        assert_eq!(parse_duration("1d"), Ok(Some(86400)));
    }

    #[test]
    fn test_parse_duration_weeks() {
        assert_eq!(parse_duration("1w"), Ok(Some(604_800)));
        assert_eq!(parse_duration("2w"), Ok(Some(1_209_600)));
    }

    #[test]
    fn test_parse_duration_years() {
        assert_eq!(parse_duration("1y"), Ok(Some(31_536_000)));
    }

    #[test]
    fn test_parse_duration_off() {
        assert_eq!(parse_duration("off"), Ok(None));
        assert_eq!(parse_duration("OFF"), Ok(None));
        assert_eq!(parse_duration("Off"), Ok(None));
    }

    #[test]
    fn test_parse_duration_bare_number() {
        assert_eq!(parse_duration("3600"), Ok(Some(3600)));
        assert_eq!(parse_duration("0"), Ok(Some(0)));
        assert_eq!(parse_duration("30"), Ok(Some(30)));
    }

    #[test]
    fn test_parse_duration_invalid_errors() {
        for val in ["", "   ", "abc", "30x", "30y5", "-30s"] {
            let res = parse_duration(val);
            assert!(res.is_err(), "{val:?} should be rejected, got {res:?}");
        }
    }

    #[test]
    fn test_parse_duration_whitespace_trimmed() {
        assert_eq!(parse_duration("  30s  "), Ok(Some(30)));
        assert_eq!(parse_duration(" off "), Ok(None));
    }

    #[test]
    fn test_resolve_static_max_age_uses_new_when_set() {
        assert_eq!(
            resolve_static_max_age(Some("1h"), None).unwrap(),
            Some(3600)
        );
    }

    #[test]
    fn test_resolve_static_max_age_falls_back_to_legacy() {
        assert_eq!(
            resolve_static_max_age(None, Some("1d")).unwrap(),
            Some(86_400)
        );
    }

    #[test]
    fn test_resolve_static_max_age_new_wins_over_legacy() {
        assert_eq!(
            resolve_static_max_age(Some("1h"), Some("2h")).unwrap(),
            Some(3600)
        );
    }

    #[test]
    fn test_resolve_static_max_age_default_is_30d() {
        assert_eq!(resolve_static_max_age(None, None).unwrap(), Some(2_592_000));
    }

    #[test]
    fn test_resolve_static_max_age_off_disables_header() {
        assert_eq!(resolve_static_max_age(Some("off"), None).unwrap(), None);
        assert_eq!(resolve_static_max_age(None, Some("off")).unwrap(), None);
    }

    #[test]
    fn test_resolve_static_max_age_garbage_errors_with_var_name() {
        let err = resolve_static_max_age(Some("garbage"), None).unwrap_err();
        assert!(err.to_string().contains("STATIC_MAX_AGE"));
        let err = resolve_static_max_age(None, Some("garbage")).unwrap_err();
        assert!(err.to_string().contains("STATIC_CACHE_TTL"));
    }

    #[test]
    fn test_resolve_static_revalidate_new_on() {
        assert!(resolve_static_revalidate(Some("on"), None).unwrap());
        assert!(resolve_static_revalidate(Some("true"), None).unwrap());
        assert!(resolve_static_revalidate(Some("1"), None).unwrap());
        assert!(resolve_static_revalidate(Some("yes"), None).unwrap());
    }

    #[test]
    fn test_resolve_static_revalidate_new_off() {
        assert!(!resolve_static_revalidate(Some("off"), None).unwrap());
        assert!(!resolve_static_revalidate(Some("false"), None).unwrap());
        assert!(!resolve_static_revalidate(Some("0"), None).unwrap());
        assert!(!resolve_static_revalidate(Some("no"), None).unwrap());
    }

    #[test]
    fn test_resolve_static_revalidate_new_garbage_errors() {
        let err = resolve_static_revalidate(Some("garbage"), None).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("STATIC_REVALIDATE"), "msg: {msg}");
        assert!(msg.contains("garbage"), "msg: {msg}");
    }

    #[test]
    fn test_resolve_static_revalidate_new_empty_errors() {
        assert!(resolve_static_revalidate(Some(""), None).is_err());
    }

    #[test]
    fn test_resolve_static_revalidate_legacy_off_means_revalidate() {
        assert!(resolve_static_revalidate(None, Some("off")).unwrap());
        assert!(resolve_static_revalidate(None, Some("OFF")).unwrap());
    }

    #[test]
    fn test_resolve_static_revalidate_legacy_other_values_no_revalidate() {
        assert!(!resolve_static_revalidate(None, Some("on")).unwrap());
        assert!(!resolve_static_revalidate(None, Some("")).unwrap());
    }

    #[test]
    fn test_resolve_static_revalidate_new_wins_over_legacy() {
        assert!(!resolve_static_revalidate(Some("off"), Some("off")).unwrap());
        assert!(resolve_static_revalidate(Some("on"), Some("anything")).unwrap());
    }

    #[test]
    fn test_resolve_static_revalidate_default_is_false() {
        assert!(!resolve_static_revalidate(None, None).unwrap());
    }
}

#[cfg(test)]
mod entry_file_tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    // Process-global env vars are touched here — serialise.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        for (k, prev_val) in prev {
            match prev_val {
                Some(v) => std::env::set_var(&k, v),
                None => std::env::remove_var(&k),
            }
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    fn make_root_with(files: &[&str]) -> TempDir {
        let dir = TempDir::new().unwrap();
        for rel in files {
            let p = dir.path().join(rel);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&p, b"<?php // test\n").unwrap();
        }
        dir
    }

    // ── resolve_entry_file ──

    #[test]
    fn resolve_entry_file_relative() {
        let dir = make_root_with(&["index.php"]);
        let resolved = resolve_entry_file("index.php", dir.path()).unwrap();
        assert_eq!(
            resolved,
            dir.path().canonicalize().unwrap().join("index.php")
        );
    }

    #[test]
    fn resolve_entry_file_absolute() {
        let dir = make_root_with(&["index.php"]);
        let abs = dir.path().join("index.php");
        let resolved = resolve_entry_file(abs.to_str().unwrap(), dir.path()).unwrap();
        assert_eq!(resolved, abs.canonicalize().unwrap());
    }

    #[test]
    fn resolve_entry_file_dotdot_escape_allowed() {
        // Worker bootstrap living outside the public document root is a
        // first-class supported layout.
        let outer = TempDir::new().unwrap();
        std::fs::write(outer.path().join("worker.php"), b"<?php // worker\n").unwrap();
        let public = outer.path().join("public");
        std::fs::create_dir_all(&public).unwrap();
        let resolved = resolve_entry_file("../worker.php", &public).unwrap();
        assert_eq!(
            resolved,
            outer.path().canonicalize().unwrap().join("worker.php")
        );
    }

    #[test]
    fn resolve_entry_file_missing_errors() {
        let dir = TempDir::new().unwrap();
        let err = resolve_entry_file("missing.php", dir.path()).unwrap_err();
        assert!(err.to_string().contains("ENTRY_FILE=missing.php"));
    }

    // ── resolve_entry_and_mode (env-driven) ──

    #[test]
    fn env_new_entry_only_no_worker_mode() {
        let dir = make_root_with(&["index.php"]);
        with_env(
            &[
                ("ENTRY_FILE", Some("index.php")),
                ("WORKER_MODE_ENABLED", None),
                ("WORKER_FILE", None),
                ("INDEX_FILE", None),
            ],
            || {
                let (entry, worker) = resolve_entry_and_mode(dir.path()).unwrap();
                assert!(entry.is_some(), "entry_file should be set");
                assert!(!worker, "worker mode should be off");
            },
        );
    }

    #[test]
    fn env_new_worker_mode_with_entry() {
        let dir = make_root_with(&["worker.php"]);
        with_env(
            &[
                ("ENTRY_FILE", Some("worker.php")),
                ("WORKER_MODE_ENABLED", Some("true")),
                ("WORKER_FILE", None),
                ("INDEX_FILE", None),
            ],
            || {
                let (entry, worker) = resolve_entry_and_mode(dir.path()).unwrap();
                assert!(entry.is_some());
                assert!(worker);
            },
        );
    }

    #[test]
    fn env_worker_mode_accepts_yes_and_1() {
        let dir = make_root_with(&["w.php"]);
        for val in ["1", "yes", "TRUE", "Yes"] {
            with_env(
                &[
                    ("ENTRY_FILE", Some("w.php")),
                    ("WORKER_MODE_ENABLED", Some(val)),
                    ("WORKER_FILE", None),
                    ("INDEX_FILE", None),
                ],
                || {
                    let (_, worker) = resolve_entry_and_mode(dir.path()).unwrap();
                    assert!(
                        worker,
                        "WORKER_MODE_ENABLED={val:?} should enable worker mode"
                    );
                },
            );
        }
    }

    #[test]
    fn env_worker_mode_explicit_falsy_disables() {
        let dir = make_root_with(&["w.php"]);
        for val in ["false", "0", "no", "off", "FALSE"] {
            with_env(
                &[
                    ("ENTRY_FILE", Some("w.php")),
                    ("WORKER_MODE_ENABLED", Some(val)),
                    ("WORKER_FILE", None),
                    ("INDEX_FILE", None),
                ],
                || {
                    let (_, worker) = resolve_entry_and_mode(dir.path()).unwrap();
                    assert!(
                        !worker,
                        "WORKER_MODE_ENABLED={val:?} should disable worker mode"
                    );
                },
            );
        }
    }

    #[test]
    fn env_worker_mode_rejects_garbage() {
        let dir = make_root_with(&["w.php"]);
        for val in ["ture", "garbage", "2"] {
            with_env(
                &[
                    ("ENTRY_FILE", Some("w.php")),
                    ("WORKER_MODE_ENABLED", Some(val)),
                    ("WORKER_FILE", None),
                    ("INDEX_FILE", None),
                ],
                || {
                    let res = resolve_entry_and_mode(dir.path());
                    let err = res
                        .err()
                        .unwrap_or_else(|| panic!("WORKER_MODE_ENABLED={val:?} should error"));
                    let msg = err.to_string();
                    assert!(
                        msg.contains("WORKER_MODE_ENABLED"),
                        "error should mention var name, got: {msg}"
                    );
                },
            );
        }
    }

    #[test]
    fn env_worker_mode_empty_uses_default() {
        // `FOO=` (empty) is treated as unset — Docker Compose substitution
        // `WORKER_MODE_ENABLED=${WORKER_MODE_ENABLED}` with the host var
        // missing must not refuse to start.
        let dir = make_root_with(&["w.php"]);
        for val in ["", "   "] {
            with_env(
                &[
                    ("ENTRY_FILE", Some("w.php")),
                    ("WORKER_MODE_ENABLED", Some(val)),
                    ("WORKER_FILE", None),
                    ("INDEX_FILE", None),
                ],
                || {
                    let (_, worker) = resolve_entry_and_mode(dir.path()).unwrap();
                    assert!(
                        !worker,
                        "WORKER_MODE_ENABLED={val:?} should fall back to default (false)"
                    );
                },
            );
        }
    }

    #[test]
    fn env_legacy_worker_file_implies_worker_mode() {
        let dir = make_root_with(&["legacy_worker.php"]);
        with_env(
            &[
                ("ENTRY_FILE", None),
                ("WORKER_MODE_ENABLED", None),
                ("WORKER_FILE", Some("legacy_worker.php")),
                ("INDEX_FILE", None),
            ],
            || {
                let (entry, worker) = resolve_entry_and_mode(dir.path()).unwrap();
                assert!(entry.is_some(), "WORKER_FILE should backfill entry_file");
                assert!(worker, "WORKER_FILE should imply worker mode");
            },
        );
    }

    #[test]
    fn env_legacy_index_file_maps_to_entry() {
        let dir = make_root_with(&["index.php"]);
        with_env(
            &[
                ("ENTRY_FILE", None),
                ("WORKER_MODE_ENABLED", None),
                ("WORKER_FILE", None),
                ("INDEX_FILE", Some("index.php")),
            ],
            || {
                let (entry, worker) = resolve_entry_and_mode(dir.path()).unwrap();
                assert!(entry.is_some());
                assert!(!worker, "INDEX_FILE alone should not enable worker mode");
            },
        );
    }

    #[test]
    fn env_new_wins_over_legacy() {
        let dir = make_root_with(&["new.php", "legacy.php"]);
        with_env(
            &[
                ("ENTRY_FILE", Some("new.php")),
                ("WORKER_MODE_ENABLED", None),
                ("WORKER_FILE", Some("legacy.php")),
                ("INDEX_FILE", Some("legacy.php")),
            ],
            || {
                let (entry, _worker) = resolve_entry_and_mode(dir.path()).unwrap();
                let entry = entry.expect("entry_file should be set");
                assert!(
                    entry.ends_with("new.php"),
                    "ENTRY_FILE must win, got {}",
                    entry.display()
                );
            },
        );
    }

    // ── Config::validate matrix ──

    fn cfg_with(entry: Option<PathBuf>, worker_mode: bool) -> Config {
        let mut c = Config::test_minimal();
        let dir = TempDir::new().unwrap();
        c.server.document_root = dir.path().to_path_buf();
        c.entry_file = entry;
        c.worker_mode_enabled = worker_mode;
        std::mem::forget(dir); // keep dir alive for the duration of validate()
        c
    }

    #[test]
    fn validate_worker_mode_without_entry_errors() {
        let cfg = cfg_with(None, true);
        let errors = cfg.validate();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("WORKER_MODE_ENABLED=true requires ENTRY_FILE")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn validate_worker_mode_with_non_php_entry_errors() {
        let dir = make_root_with(&["index.html"]);
        let cfg = cfg_with(Some(dir.path().join("index.html")), true);
        let errors = cfg.validate();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("requires a .php ENTRY_FILE")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn validate_worker_mode_with_php_entry_ok() {
        let dir = make_root_with(&["worker.php"]);
        let cfg = cfg_with(Some(dir.path().join("worker.php")), true);
        let errors = cfg.validate();
        assert!(
            errors.iter().all(|e| !e.contains("WORKER_MODE_ENABLED")),
            "got worker-mode errors: {errors:?}"
        );
    }

    #[test]
    fn validate_traditional_no_entry_ok() {
        let cfg = cfg_with(None, false);
        let errors = cfg.validate();
        assert!(
            errors
                .iter()
                .all(|e| !e.contains("ENTRY_FILE") && !e.contains("WORKER_MODE_ENABLED")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn validate_framework_php_entry_ok() {
        let dir = make_root_with(&["index.php"]);
        let cfg = cfg_with(Some(dir.path().join("index.php")), false);
        let errors = cfg.validate();
        assert!(
            errors.iter().all(|e| !e.contains("WORKER_MODE_ENABLED")),
            "got: {errors:?}"
        );
    }

    #[test]
    fn validate_spa_html_entry_ok() {
        let dir = make_root_with(&["index.html"]);
        let cfg = cfg_with(Some(dir.path().join("index.html")), false);
        let errors = cfg.validate();
        assert!(
            errors.iter().all(|e| !e.contains("WORKER_MODE_ENABLED")),
            "got: {errors:?}"
        );
    }
}
