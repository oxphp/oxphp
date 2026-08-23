mod env_bool;
mod php_deny;
mod proxy;
mod server;
pub(crate) mod symlink_allow;
#[cfg(test)]
pub(crate) mod test_env;
mod tls;
mod workers;

use std::fmt;
use std::path::{Path, PathBuf};

use crate::server::compression::{Coding, Levels};
#[allow(unused_imports)] // consumed by feature-gated plugins
pub(crate) use env_bool::parse_bool_opt;
pub(crate) use env_bool::{parse_bool_strict, parse_env_bool};
pub use php_deny::{DeniedMeta, DenyFallback, PhpDeny, RoutingModeKind};
pub use proxy::{
    classify_bind_exposure, parse_cidr_list, BindExposure, IpAllowList, TrustedProxyConfig,
};
pub use server::{H2Config, ServerConfig};
pub use symlink_allow::SymlinkAllowList;
pub use tls::TlsMinVersion;
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
    /// Minimum accepted TLS protocol version. Parsed and validated at startup
    /// even when TLS is not enabled, so a typo'd floor fails loudly everywhere
    /// (including `oxphp config --check`), not just once TLS is turned on.
    pub tls_min_version: TlsMinVersion,
    pub error_pages_dir: Option<String>,
    /// Per-coding compression level, already resolved against
    /// `COMPRESSION_ENCODINGS`: a coding left out of the offered set arrives
    /// here as zero, which is the one representation the server reads.
    pub compression: Levels,
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
    /// How long a request may wait for a free queue slot before it is shed
    /// with 529. `0` = fail fast (shed the moment the queue is full).
    pub queue_wait_timeout_ms: u64,
    /// Cap on requests parked waiting for a queue slot.
    pub queue_max_waiting: usize,
    /// Cap on the request-body bytes those parked requests may hold between
    /// them. Bodies are buffered in full before dispatch, so the cap above
    /// bounds the waiting set in requests and this one bounds it in memory.
    pub queue_max_waiting_bytes: usize,
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

/// Parse a numeric env knob fail-closed: unset or exactly-empty (`${VAR:-}`
/// substitution) yields `default`, anything that is not a non-negative
/// integer is a hard error naming the variable. A silent fallback would let
/// a typo quietly change pool sizing long after startup.
fn parse_knob(name: &str, default: usize) -> Result<usize, crate::types::BoxError> {
    match std::env::var(name) {
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name} is not valid UTF-8").into()),
        Ok(v) if v.is_empty() => Ok(default),
        Ok(v) => v
            .trim()
            .parse::<usize>()
            .map_err(|_| format!("{name} must be a non-negative integer (got {v:?})").into()),
    }
}

/// One coding's compression level. Unset or empty (`${VAR:-}` substitution)
/// yields `default`; a value outside the coding's range is a hard error rather
/// than a clamp, since a clamped level looks exactly like a working one in
/// every log line afterwards.
fn compression_level(name: &str, max: i32, default: i32) -> Result<i32, crate::types::BoxError> {
    parse_compression_level(name, max, default, std::env::var(name))
}

/// The reading half of [`compression_level`], split out so the range and the
/// unreadable-value rejection can be tested without mutating the environment.
fn parse_compression_level(
    name: &str,
    max: i32,
    default: i32,
    raw: Result<String, std::env::VarError>,
) -> Result<i32, crate::types::BoxError> {
    match raw {
        Err(std::env::VarError::NotPresent) => Ok(default),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name} is not valid UTF-8").into()),
        Ok(val) if val.trim().is_empty() => Ok(default),
        Ok(val) => match val.trim().parse::<i32>() {
            Ok(level) if (0..=max).contains(&level) => Ok(level),
            _ => Err(format!(
                "{name} must be 0-{max} (got {val:?}), 0 = the coding is not offered"
            )
            .into()),
        },
    }
}

/// Read the compression configuration out of the environment.
fn compression_from_env() -> Result<Levels, crate::types::BoxError> {
    let levels = Levels {
        // Brotli's quality knee sits between 4 and 5, where it changes hasher,
        // and 5 is the level at which preferring brotli over gzip pays for
        // itself: at 4 it produced more bytes than gzip's own default on JSON
        // above 4 KB and on real minified assets, and spent more CPU doing it.
        brotli: compression_level("COMPRESSION_BROTLI_LEVEL", 11, 5)?,
        // Level 6 is zlib's own default and the point the benchmark put the
        // knee at: on real assets level 9 costs about twice as much for a
        // percent or two of size.
        gzip: compression_level("COMPRESSION_GZIP_LEVEL", 9, 6)?,
        // Level 6 rather than zstd's own default of 3: on bodies over 4 KB it
        // produces fewer bytes in less time than the brotli quality earlier
        // releases compressed everything with, so no deployment sends more
        // bytes after the upgrade than before it.
        zstd: compression_level("COMPRESSION_ZSTD_LEVEL", 19, 6)?,
    };
    resolve_compression(
        text_knob("COMPRESSION_ENCODINGS")?.as_deref(),
        text_knob("COMPRESSION_LEVEL")?.as_deref(),
        levels,
        text_knob("COMPRESSION_BROTLI_LEVEL")?.is_some(),
    )
}

/// Read a compression knob whose value is text rather than a number. An
/// exactly-empty value is an unset one, the way it is for every other knob here
/// — a `${VAR:-}` substitution must not abort startup — but an unreadable one
/// is not: a silent default there is the one outcome an operator has no way to
/// notice, which is exactly what the numeric knobs refuse.
fn text_knob(name: &str) -> Result<Option<String>, crate::types::BoxError> {
    parse_text_knob(name, std::env::var(name))
}

/// The reading half of [`text_knob`], split out the same way and for the same
/// reason as [`parse_compression_level`].
fn parse_text_knob(
    name: &str,
    raw: Result<String, std::env::VarError>,
) -> Result<Option<String>, crate::types::BoxError> {
    match raw {
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name} is not valid UTF-8").into()),
        Ok(val) if val.trim().is_empty() => Ok(None),
        Ok(val) => Ok(Some(val)),
    }
}

/// Decide which codings this server offers, and at what level each one runs.
///
/// `COMPRESSION_ENCODINGS` chooses the set and the level variables choose the
/// effort; the two compose by AND, so a coding is produced when it is listed
/// and its level is not zero. The order codings are listed in carries no
/// meaning: which one a client is served depends on whether the compressed
/// bytes are kept and reused or thrown away after the response, and no single
/// ordering can express both.
fn resolve_compression(
    encodings: Option<&str>,
    legacy_level: Option<&str>,
    mut levels: Levels,
    brotli_level_set: bool,
) -> Result<Levels, crate::types::BoxError> {
    if let Some(val) = legacy_level {
        let level = match val.trim().parse::<i32>() {
            Ok(level) if (0..=11).contains(&level) => level,
            _ => {
                return Err(
                    format!("COMPRESSION_LEVEL must be 0-11 (got {val:?}), 0 = disabled").into(),
                );
            }
        };
        tracing::warn!(
            "COMPRESSION_LEVEL is deprecated: use COMPRESSION_BROTLI_LEVEL for brotli's quality and COMPRESSION_ENCODINGS to choose which codings are offered"
        );
        if level == 0 {
            // The switch it has always been. A deployment that set it to turn
            // compression off must not start emitting zstd because this server
            // learned a coding it had never heard of.
            return Ok(Levels::default());
        }
        // An explicit COMPRESSION_BROTLI_LEVEL is the newer statement of the
        // same thing, so it wins over the variable it replaces.
        if !brotli_level_set {
            levels.brotli = level;
        }
    }

    let Some(val) = encodings.map(str::trim).filter(|val| !val.is_empty()) else {
        return Ok(levels);
    };
    let mut offered = Levels::default();
    if !val.eq_ignore_ascii_case("off") && !val.eq_ignore_ascii_case("none") {
        for token in val.split(',').map(str::trim).filter(|t| !t.is_empty()) {
            let Some(coding) = Coding::from_name(token) else {
                return Err(format!(
                    "COMPRESSION_ENCODINGS: unknown coding {token:?} — expected a comma-separated list of br, gzip, zstd, or \"off\""
                )
                .into());
            };
            offered.set(coding, levels.level(coding));
        }
    }
    Ok(offered)
}

/// Default cap on the bodies parked in the waiting set, in bytes.
///
/// Chosen against the places cap rather than against host memory, which is
/// unknowable here: at the default `QUEUE_MAX_WAITING` of `workers × 128` this
/// is some tens of kilobytes per waiter — above an ordinary form post, and far
/// below what the same set of waiters could hold with only a count to stop
/// them (`QUEUE_MAX_WAITING` × the 10 MiB per-request body limit, which reaches
/// gigabytes at any pool size worth running).
const DEFAULT_QUEUE_MAX_WAITING_BYTES: usize = 64 * 1024 * 1024;

/// Parse the PHP queue knobs — `QUEUE_CAPACITY`, `QUEUE_WAIT_TIMEOUT_MS`,
/// `QUEUE_MAX_WAITING` and `QUEUE_MAX_WAITING_BYTES`.
///
/// `QUEUE_CAPACITY=0` means auto (`worker_count × 128`), matching
/// `ASYNC_QUEUE_CAPACITY`. Taken literally it would build a zero-capacity
/// rendezvous channel in which a request could only be handed over if a
/// worker happened to be blocked waiting at that exact moment — never what
/// an operator writing `0` intends.
pub(crate) fn resolve_queue_env(
    worker_count: usize,
    max_connections: usize,
) -> Result<(usize, u64, usize, usize), crate::types::BoxError> {
    let capacity = match parse_knob("QUEUE_CAPACITY", 0)? {
        0 => worker_count * 128,
        n => n,
    };
    let wait_timeout_ms = parse_knob("QUEUE_WAIT_TIMEOUT_MS", 1000)? as u64;
    // How many may wait at once — a bound on resources held, not on waits that
    // will pay off (that would be `service_rate × budget`, unknowable here).
    // Generous by default so fast handlers are not refused out of the box; the
    // configuration reference gives slow ones the arithmetic to size it.
    //
    // `worker_count`, not `capacity`: deriving it from the operator's
    // `QUEUE_CAPACITY` would refuse burst absorption exactly where the queue is
    // shallow. The `MAX_CONNECTIONS` ceiling bounds this part of the backlog
    // only — what keeps the accept loop fed is the check below, over all three.
    let max_waiting = match parse_knob("QUEUE_MAX_WAITING", 0)? {
        0 => (worker_count * 128).min(max_connections / 2).max(1),
        n => n,
    };
    // The other half of the same cap. A place in the waiting set is also a
    // buffered request body held for the whole budget, and the count says
    // nothing about how large those bodies are.
    let max_waiting_bytes = match parse_knob("QUEUE_MAX_WAITING_BYTES", 0)? {
        0 => DEFAULT_QUEUE_MAX_WAITING_BYTES,
        n => n,
    };
    if let Some(backlog) =
        php_backlog_over_connection_budget(worker_count, capacity, max_waiting, max_connections)
    {
        tracing::warn!(
            php_workers = worker_count,
            queue_capacity = capacity,
            queue_max_waiting = max_waiting,
            max_connections,
            backlog,
            "the PHP path alone can hold every allowed connection — under sustained \
             overload the accept loop stops accepting and clients get no response at \
             all instead of 529. Lower QUEUE_CAPACITY, or set QUEUE_MAX_WAITING \
             explicitly and raise MAX_CONNECTIONS past the backlog: raising it on its \
             own does not clear this, because the default waiting set is half of it. \
             Expected on an HTTP/2-heavy deployment, where one connection carries many \
             requests"
        );
    }
    Ok((capacity, wait_timeout_ms, max_waiting, max_waiting_bytes))
}

/// How many requests the PHP path can hold without answering — running in a
/// worker, sitting in the queue, or parked in admission — when that is enough
/// to take every connection the server is allowed to have.
///
/// All three hold a connection, and therefore a `MAX_CONNECTIONS` permit, until
/// their request is answered. They are three separate populations because a
/// queue slot is released the moment a worker picks the request up, before the
/// script runs: a running request occupies neither buffer and still holds its
/// connection. `QUEUE_MAX_WAITING`'s own `MAX_CONNECTIONS / 2` ceiling covers
/// one of the three.
///
/// Past that sum the accept loop is the thing that stops: it takes a permit
/// before spawning a connection task, so it parks with a connection already
/// accepted and answers nothing — a worse signal than the 529 the wait budget
/// exists to produce, because a balancer cannot tell it from a dead node.
///
/// **Necessary, not sufficient.** The running term is `worker_count`, which is
/// a floor rather than a bound: a dynamic pool grows past its initial count,
/// and in worker mode one thread multiplexes fibers, so it can hold many
/// unanswered requests at once. Connections that never reach PHP at all — idle
/// keep-alives, static files, handshakes in progress — take permits too and
/// cannot be counted at startup. A configuration this clears can still exhaust
/// the budget; one it flags is exhausting it by configuration alone.
///
/// The terms count connections while the budget is spent by requests, so over
/// HTTP/2 — one connection carrying many streams — exceeding it can be
/// intentional.
pub(crate) fn php_backlog_over_connection_budget(
    worker_count: usize,
    capacity: usize,
    max_waiting: usize,
    max_connections: usize,
) -> Option<usize> {
    let backlog = worker_count
        .saturating_add(capacity)
        .saturating_add(max_waiting);
    (backlog >= max_connections).then_some(backlog)
}

/// Parse the async-pool env triple — `ASYNC_WORKERS`, `ASYNC_QUEUE_CAPACITY`,
/// `ASYNC_MAX_FIBERS`. Shared by `Config::from_env` and the one-shot CLI
/// path, which reads only the variables it actually consumes.
///
/// Unset or exactly-empty values fall back to the defaults `(0, 0, 256)`;
/// anything else must parse as a non-negative integer. A malformed value is
/// a hard error, not a silent fallback — `ASYNC_WORKERS=8x` collapsing to
/// `0` would quietly disable the async pool. `ASYNC_MAX_FIBERS=0` keeps its
/// historical meaning of "use the default cap" (256).
pub(crate) fn resolve_async_pool_env() -> Result<(usize, usize, usize), crate::types::BoxError> {
    let async_workers = parse_knob("ASYNC_WORKERS", 0)?;
    let async_queue_capacity = parse_knob("ASYNC_QUEUE_CAPACITY", 0)?;
    let async_max_fibers = match parse_knob("ASYNC_MAX_FIBERS", 256)? {
        0 => 256,
        n => n,
    };
    Ok((async_workers, async_queue_capacity, async_max_fibers))
}

/// Optional env var that must be UTF-8 when present: unset — or *exactly*
/// empty, per the codebase convention for `${VAR:-}`-style compose/Helm
/// substitutions — → `None`, non-UTF-8 → hard error. A plain `var(..).ok()`
/// would turn a corrupted value into "TLS silently disabled" — a worse
/// downgrade than the invalid-value class `TLS_MIN_VERSION` already rejects —
/// and an empty pair into an `fs::read("")` crash at startup.
///
/// Whitespace-only values deliberately stay `Some`: `" "` is never a valid
/// TLS path, and collapsing it to "unset" would fail open (plain HTTP on an
/// intended-HTTPS port when a secret mount emits a stray space/newline).
/// Left as-is it fails closed downstream — unreadable path or half-pair
/// abort.
pub(crate) fn optional_utf8_env(name: &str) -> Result<Option<String>, crate::types::BoxError> {
    match std::env::var(name) {
        Ok(v) if v.is_empty() => Ok(None),
        Ok(v) => Ok(Some(v)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(format!("{name} is not valid UTF-8").into()),
    }
}

impl Config {
    pub fn from_env() -> Result<Self, crate::types::BoxError> {
        let server = ServerConfig::from_env()?;
        let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let executor_type = std::env::var("EXECUTOR")
            .unwrap_or_else(|_| "sapi".to_string())
            .to_ascii_lowercase();
        // Strict like the queue knobs it now feeds: `MAX_CONNECTIONS=500x`
        // falling back to 10 000 silently would also move the computed
        // `QUEUE_MAX_WAITING` ceiling, so a typo here reshapes admission.
        let max_connections = parse_knob("MAX_CONNECTIONS", 10_000)?;
        // Default 25s, not 30s: shutdown takes up to drain + ~2s of forced
        // unwind + the plugin flush, and the whole sequence must fit inside
        // the orchestrator's kill window — Kubernetes defaults
        // terminationGracePeriodSeconds to 30. 25 + 2 + flush leaves headroom;
        // a 30s default would be SIGKILLed mid-flush on stock deployments.
        let drain_timeout_seconds = std::env::var("DRAIN_TIMEOUT_SECONDS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(25);
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
        let tls_cert = optional_utf8_env("TLS_CERT")?;
        let tls_key = optional_utf8_env("TLS_KEY")?;
        // A `${VAR:-}` substitution that rendered empty (broken secret
        // mount?) leaves a breadcrumb: the environment *mentions* TLS, yet
        // the server is about to serve plaintext. Genuinely absent vars stay
        // silent — plain HTTP is the normal default. A half-pair is handled
        // separately (startup abort).
        if tls_cert.is_none() && tls_key.is_none() {
            let empty_present: Vec<&str> = ["TLS_CERT", "TLS_KEY"]
                .into_iter()
                .filter(|k| std::env::var_os(k).is_some())
                .collect();
            if !empty_present.is_empty() {
                tracing::warn!(
                    vars = empty_present.join(", "),
                    "TLS variable(s) set but empty — TLS disabled, serving plain HTTP"
                );
            }
        }
        let tls_min_version = TlsMinVersion::from_env()?;
        let error_pages_dir = std::env::var("ERROR_PAGES_DIR").ok();
        let compression = compression_from_env()?;
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

        let (async_workers, async_queue_capacity, async_max_fibers) = resolve_async_pool_env()?;

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

        let (queue_capacity, queue_wait_timeout_ms, queue_max_waiting, queue_max_waiting_bytes) =
            resolve_queue_env(worker_mode.worker_count(), max_connections)?;

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
            tls_min_version,
            error_pages_dir,
            compression,
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
            queue_wait_timeout_ms,
            queue_max_waiting,
            queue_max_waiting_bytes,
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
            drain_timeout_seconds: 25,
            internal_addr: None,
            internal_allow_ips: None,
            rate_limit: 0,
            rate_window_seconds: 60,
            tls_cert: None,
            tls_key: None,
            tls_min_version: TlsMinVersion::V12,
            error_pages_dir: None,
            compression: Levels {
                brotli: 5,
                gzip: 6,
                zstd: 6,
            },
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
            queue_wait_timeout_ms: 1000,
            queue_max_waiting: 128,
            queue_max_waiting_bytes: 64 * 1024 * 1024,
            trusted_proxies: None,
            h2: H2Config::default(),
        }
    }

    /// Canonical error for a half-configured TLS pair — exactly one of
    /// `TLS_CERT`/`TLS_KEY` set, which is almost always a typo'd variable
    /// name and must never silently serve plain HTTP. One string shared by
    /// `validate()` (`config --check`) and the serve-startup abort, so the
    /// two reports cannot drift.
    pub fn half_configured_tls_error(&self) -> Option<String> {
        let (set, missing) = match (self.tls_cert.is_some(), self.tls_key.is_some()) {
            (true, false) => ("TLS_CERT", "TLS_KEY"),
            (false, true) => ("TLS_KEY", "TLS_CERT"),
            _ => return None,
        };
        Some(format!(
            "{set} is set but {missing} is missing — both are required to enable TLS (unset {set} to serve plain HTTP)"
        ))
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
            "queue_wait_timeout_ms": self.queue_wait_timeout_ms,
            "queue_max_waiting": self.queue_max_waiting,
            "queue_max_waiting_bytes": self.queue_max_waiting_bytes,
            "max_connections": self.max_connections,
            "drain_timeout_seconds": self.drain_timeout_seconds,
            "internal_addr": self.internal_addr,
            "header_timeout_seconds": self.server.header_read_timeout.as_secs(),
            "rate_limit": self.rate_limit,
            "rate_window_seconds": self.rate_window_seconds,
            "tls_enabled": self.tls_cert.is_some() && self.tls_key.is_some(),
            "tls_min_version": self.tls_min_version.to_string(),
            "error_pages_dir": self.error_pages_dir,
            "brotli_level": self.compression.brotli,
            "gzip_level": self.compression.gzip,
            "zstd_level": self.compression.zstd,
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
    /// `TLS_CERT`/`TLS_KEY` must be set as a pair — a typo'd variable name is
    /// caught here pre-deploy, and `serve` aborts on the same half-pair.
    /// All problems are collected — the function never short-circuits.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        check_dir("DOCUMENT_ROOT", &self.server.document_root, &mut errors);
        if let Some(err) = self.half_configured_tls_error() {
            errors.push(err);
        }
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

    /// What the level variables parse to when none of them are set.
    fn shipped_levels() -> Levels {
        Levels {
            brotli: 5,
            gzip: 6,
            zstd: 6,
        }
    }

    fn resolve(encodings: Option<&str>, legacy: Option<&str>) -> Levels {
        resolve_compression(encodings, legacy, shipped_levels(), false).expect("valid")
    }

    #[test]
    fn a_level_outside_its_own_coding_range_is_a_startup_error() {
        // Each ceiling is the coding's own — gzip stops at 9, Zstandard at 19 —
        // and a clamp would read as a working configuration in every log line
        // afterwards.
        for (name, max, over) in [
            ("COMPRESSION_GZIP_LEVEL", 9, "10"),
            ("COMPRESSION_ZSTD_LEVEL", 19, "20"),
            ("COMPRESSION_BROTLI_LEVEL", 11, "12"),
        ] {
            let err = parse_compression_level(name, max, 6, Ok(over.to_string()))
                .expect_err("above the ceiling");
            assert!(err.to_string().contains(name), "{err}");
            assert!(parse_compression_level(name, max, 6, Ok("-1".into())).is_err());
            assert_eq!(
                parse_compression_level(name, max, 6, Ok(max.to_string())).unwrap(),
                max
            );
        }
        // Unset and `${VAR:-}` alike leave the default in place.
        let unset = Err(std::env::VarError::NotPresent);
        assert_eq!(
            parse_compression_level("COMPRESSION_GZIP_LEVEL", 9, 6, unset).unwrap(),
            6
        );
        assert_eq!(
            parse_compression_level("COMPRESSION_GZIP_LEVEL", 9, 6, Ok("  ".into())).unwrap(),
            6
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_compression_value_that_is_not_valid_utf8_is_a_startup_error() {
        // Reading it as unset would quietly pick the default — the one outcome
        // an operator has no way to notice, and the reason every numeric knob
        // here rejects it by name.
        use std::os::unix::ffi::OsStringExt;
        let unreadable = || {
            Err(std::env::VarError::NotUnicode(
                std::ffi::OsString::from_vec(vec![0xff]),
            ))
        };

        let err = parse_compression_level("COMPRESSION_BROTLI_LEVEL", 11, 5, unreadable())
            .expect_err("an unreadable level is not a default");
        assert!(
            err.to_string().contains("COMPRESSION_BROTLI_LEVEL"),
            "{err}"
        );

        // The two text-valued knobs read the same way — they were the pair
        // that still fell back.
        for name in ["COMPRESSION_ENCODINGS", "COMPRESSION_LEVEL"] {
            let err = parse_text_knob(name, unreadable()).expect_err("nor is an unreadable set");
            assert!(err.to_string().contains(name), "{err}");
        }
        assert_eq!(
            parse_text_knob("COMPRESSION_ENCODINGS", Ok(" ".into())).unwrap(),
            None
        );
    }

    #[test]
    fn compression_offers_every_coding_by_default() {
        let levels = resolve(None, None);
        for coding in Coding::ALL {
            assert!(levels.level(coding) > 0, "{} is not offered", coding.name());
        }
    }

    #[test]
    fn compression_encodings_withdraws_the_codings_it_leaves_out() {
        let levels = resolve(Some("zstd, gzip"), None);
        assert_eq!(levels.brotli, 0);
        assert_eq!(levels.gzip, 6);
        assert_eq!(levels.zstd, 6);
        // Spelled the way a settings file reads rather than the way the header
        // does, and in whatever case and spacing it was typed.
        assert_eq!(resolve(Some(" BROTLI "), None).brotli, 5);
        assert_eq!(resolve(Some(" BROTLI "), None).zstd, 0);
    }

    #[test]
    fn compression_can_be_switched_off_without_the_deprecated_variable() {
        for spelling in ["off", "none", "OFF"] {
            let levels = resolve(Some(spelling), None);
            for coding in Coding::ALL {
                assert_eq!(levels.level(coding), 0, "{spelling} left {coding:?} on");
            }
        }
    }

    #[test]
    fn an_empty_encodings_list_is_an_unset_one() {
        // `COMPRESSION_ENCODINGS=${VAR:-}` must not silently turn compression
        // off — "off" is how that is asked for.
        assert_eq!(resolve(Some(""), None).zstd, 6);
        assert_eq!(resolve(Some("  "), None).zstd, 6);
    }

    #[test]
    fn an_unknown_coding_is_a_startup_error() {
        // A typo that quietly withdrew a coding would show up as a bandwidth
        // bill, months later.
        let err = resolve_compression(Some("zstd, deflate"), None, shipped_levels(), false)
            .expect_err("deflate is not a coding this server produces");
        assert!(err.to_string().contains("deflate"), "{err}");
    }

    #[test]
    fn a_listed_coding_at_level_zero_is_still_not_offered() {
        let levels = Levels {
            brotli: 0,
            ..shipped_levels()
        };
        let resolved = resolve_compression(Some("br, zstd"), None, levels, true).expect("valid");
        assert_eq!(resolved.brotli, 0);
        assert_eq!(resolved.zstd, 6);
    }

    #[test]
    fn the_deprecated_level_variable_keeps_both_of_its_meanings() {
        // Zero meant "no compression at all" when brotli was the only coding,
        // and a deployment that set it must not start emitting zstd.
        let off = resolve(None, Some("0"));
        for coding in Coding::ALL {
            assert_eq!(off.level(coding), 0, "{coding:?} survived the off switch");
        }
        // Anything else named brotli's quality, and still does.
        let raised = resolve(None, Some("9"));
        assert_eq!(raised.brotli, 9);
        assert_eq!(raised.zstd, 6);
    }

    #[test]
    fn an_explicit_brotli_level_wins_over_the_variable_it_replaces() {
        let levels = Levels {
            brotli: 11,
            ..shipped_levels()
        };
        let resolved = resolve_compression(None, Some("2"), levels, true).expect("valid");
        assert_eq!(resolved.brotli, 11);
    }

    #[test]
    fn an_out_of_range_legacy_level_is_still_rejected() {
        let err = resolve_compression(None, Some("12"), shipped_levels(), false)
            .expect_err("brotli quality stops at 11");
        assert!(err.to_string().contains("COMPRESSION_LEVEL"), "{err}");
    }

    #[test]
    fn validate_flags_half_configured_tls() {
        let mut config = Config::test_minimal();
        config.tls_cert = Some("/etc/ssl/cert.pem".to_string());
        let errors = config.validate();
        assert!(
            errors
                .iter()
                .any(|e| e.contains("TLS_KEY is missing") && e.contains("TLS_CERT")),
            "errors: {errors:?}"
        );
    }

    // Both tests exercise `optional_utf8_env` directly rather than the full
    // `Config::from_env`, which reads the whole ambient environment — an
    // unrelated invalid var in a CI shell would turn these into false reds.

    #[test]
    fn empty_tls_cert_and_key_are_treated_as_unset() {
        // `${TLS_CERT:-}`-style substitution: no false "TLS_CERT is set but
        // TLS_KEY is missing" error, no `fs::read("")` crash.
        test_env::with_env(&[("TLS_CERT", Some("")), ("TLS_KEY", Some(""))], || {
            assert_eq!(optional_utf8_env("TLS_CERT").unwrap(), None);
            assert_eq!(optional_utf8_env("TLS_KEY").unwrap(), None);
        });
    }

    #[test]
    fn max_connections_is_parsed_as_strictly_as_the_knobs_it_feeds() {
        // Not cosmetic consistency: the default `QUEUE_MAX_WAITING` is capped
        // at half of this value, so a silent fallback here silently reshapes
        // admission as well as the connection limit.
        test_env::with_env(&[("MAX_CONNECTIONS", Some("500x"))], || {
            let err = Config::from_env().unwrap_err();
            assert!(err.to_string().contains("MAX_CONNECTIONS"), "err: {err}");
        });
        test_env::with_env(&[("MAX_CONNECTIONS", Some("500"))], || {
            assert_eq!(Config::from_env().unwrap().max_connections, 500);
        });
    }

    #[test]
    fn async_pool_env_strictness() {
        const ASYNC_VARS: [&str; 3] = ["ASYNC_WORKERS", "ASYNC_QUEUE_CAPACITY", "ASYNC_MAX_FIBERS"];
        let cleared: Vec<(&str, Option<&str>)> = ASYNC_VARS.iter().map(|k| (*k, None)).collect();

        // Unset → defaults.
        test_env::with_env(&cleared, || {
            assert_eq!(resolve_async_pool_env().unwrap(), (0, 0, 256));
        });
        // Exactly-empty = unset (`${VAR:-}` substitution); 0 fiber cap keeps
        // its historical meaning of "default".
        test_env::with_env(
            &[
                ("ASYNC_WORKERS", Some("")),
                ("ASYNC_QUEUE_CAPACITY", Some("")),
                ("ASYNC_MAX_FIBERS", Some("0")),
            ],
            || {
                assert_eq!(resolve_async_pool_env().unwrap(), (0, 0, 256));
            },
        );
        // Garbage is a hard error naming the variable — not a silently
        // disabled pool.
        test_env::with_env(
            &[
                ("ASYNC_WORKERS", Some("8x")),
                ("ASYNC_QUEUE_CAPACITY", None),
                ("ASYNC_MAX_FIBERS", None),
            ],
            || {
                let err = resolve_async_pool_env().unwrap_err();
                assert!(err.to_string().contains("ASYNC_WORKERS"), "err: {err}");
            },
        );
    }

    #[test]
    fn queue_env_strictness() {
        // Exercised through `resolve_queue_env` rather than `Config::from_env`,
        // which reads the whole ambient environment — an unrelated invalid var
        // in a CI shell would turn this into a false red. Worker count is
        // passed in, so the auto mapping is checked against a known value.
        let both_unset = [("QUEUE_CAPACITY", None), ("QUEUE_WAIT_TIMEOUT_MS", None)];

        test_env::with_env(&both_unset, || {
            assert_eq!(
                resolve_queue_env(7, 10_000).unwrap(),
                (896, 1000, 896, DEFAULT_QUEUE_MAX_WAITING_BYTES)
            );
        });
        // `0` and exactly-empty both mean auto, not a rendezvous channel.
        test_env::with_env(&[("QUEUE_CAPACITY", Some("0"))], || {
            assert_eq!(resolve_queue_env(7, 10_000).unwrap().0, 896);
        });
        test_env::with_env(&[("QUEUE_CAPACITY", Some(""))], || {
            assert_eq!(resolve_queue_env(2, 10_000).unwrap().0, 256);
        });
        test_env::with_env(&[("QUEUE_CAPACITY", Some("1"))], || {
            assert_eq!(resolve_queue_env(7, 10_000).unwrap().0, 1);
        });
        // Garbage is a hard error naming the variable, not a silent fallback
        // to a queue depth the operator never asked for.
        test_env::with_env(&[("QUEUE_CAPACITY", Some("896x"))], || {
            let err = resolve_queue_env(7, 10_000).unwrap_err();
            assert!(err.to_string().contains("QUEUE_CAPACITY"), "err: {err}");
        });

        // 0 is meaningful here — fail fast, rejecting the moment the queue is
        // full, which is how the server behaved before admission control.
        test_env::with_env(&[("QUEUE_WAIT_TIMEOUT_MS", Some("0"))], || {
            assert_eq!(resolve_queue_env(7, 10_000).unwrap().1, 0);
        });
        test_env::with_env(&[("QUEUE_WAIT_TIMEOUT_MS", Some("2s"))], || {
            let err = resolve_queue_env(7, 10_000).unwrap_err();
            assert!(
                err.to_string().contains("QUEUE_WAIT_TIMEOUT_MS"),
                "err: {err}"
            );
        });

        // Scaled by the pool, not by the connection budget: the cap has to be
        // reachable, or `waiting_full` never fires and every refusal arrives as
        // an expired budget instead.
        test_env::with_env(&[("QUEUE_MAX_WAITING", None)], || {
            assert_eq!(resolve_queue_env(7, 10_000).unwrap().2, 896);
            // The connection budget is still a bound, and the tighter of the
            // two wins. Half of MAX_CONNECTIONS is not by itself headroom for
            // the accept loop, though — queued and running requests hold
            // connections too, and neither has a ceiling of its own; see
            // `php_path_alone_can_take_the_whole_connection_budget`.
            assert_eq!(resolve_queue_env(7, 400).unwrap().2, 200);
            // Never zero: a cap of 0 would turn every contended request into a
            // shed and silently disable waiting altogether.
            assert_eq!(resolve_queue_env(7, 1).unwrap().2, 1);
            assert_eq!(resolve_queue_env(0, 10_000).unwrap().2, 1);
        });
        test_env::with_env(&[("QUEUE_MAX_WAITING", Some("32"))], || {
            assert_eq!(resolve_queue_env(7, 10_000).unwrap().2, 32);
        });
        test_env::with_env(&[("QUEUE_MAX_WAITING", Some("lots"))], || {
            let err = resolve_queue_env(7, 10_000).unwrap_err();
            assert!(err.to_string().contains("QUEUE_MAX_WAITING"), "err: {err}");
        });

        // The byte half of the same cap. Flat rather than pool-derived: what a
        // host can hold in buffered bodies has nothing to do with how many
        // threads it runs.
        test_env::with_env(&[("QUEUE_MAX_WAITING_BYTES", None)], || {
            assert_eq!(
                resolve_queue_env(7, 10_000).unwrap().3,
                DEFAULT_QUEUE_MAX_WAITING_BYTES
            );
            assert_eq!(
                resolve_queue_env(64, 10_000).unwrap().3,
                DEFAULT_QUEUE_MAX_WAITING_BYTES
            );
        });
        test_env::with_env(&[("QUEUE_MAX_WAITING_BYTES", Some("1048576"))], || {
            assert_eq!(resolve_queue_env(7, 10_000).unwrap().3, 1024 * 1024);
        });
        test_env::with_env(&[("QUEUE_MAX_WAITING_BYTES", Some("64MiB"))], || {
            let err = resolve_queue_env(7, 10_000).unwrap_err();
            assert!(
                err.to_string().contains("QUEUE_MAX_WAITING_BYTES"),
                "a size suffix is not accepted here, and saying so beats \
                 falling back to a default the operator did not ask for: {err}"
            );
        });
    }

    #[test]
    fn php_path_alone_can_take_the_whole_connection_budget() {
        // Measured before this check existed: PHP_WORKERS=1, QUEUE_CAPACITY=4,
        // QUEUE_MAX_WAITING=4, MAX_CONNECTIONS=8. Nine concurrent requests
        // pinned all eight permits and a further client got no response at all
        // (curl exit 28) where the same load under MAX_CONNECTIONS=64 was
        // refused with 529 in 3 ms.
        assert_eq!(php_backlog_over_connection_budget(1, 4, 4, 8), Some(9));
        assert_eq!(php_backlog_over_connection_budget(1, 4, 4, 64), None);
        // The running request is a third population, not a rounding error: its
        // queue slot was released at pickup, so it is in neither buffer and
        // still holds its connection. Counting only the two buffers passes this
        // configuration, which pins all nine permits exactly as the RED did.
        assert_eq!(php_backlog_over_connection_budget(1, 4, 4, 9), Some(9));
        // Equality already loses: the last permit the PHP path can take is the
        // one the accept loop needs to answer anybody else.
        assert_eq!(
            php_backlog_over_connection_budget(7, 493, 500, 1000),
            Some(1000)
        );
        assert_eq!(php_backlog_over_connection_budget(7, 492, 500, 1000), None);
        // A sum past `usize` must report the hazard, not wrap into safety.
        assert_eq!(
            php_backlog_over_connection_budget(1, usize::MAX, 1, 10),
            Some(usize::MAX)
        );

        // Reachable through the defaults, because `QUEUE_CAPACITY` does not
        // follow `MAX_CONNECTIONS` down: 7 workers give a queue of 896 against
        // a waiting set the ceiling holds to 500 — 1403 against a budget of
        // 1000.
        test_env::with_env(
            &[
                ("QUEUE_CAPACITY", None),
                ("QUEUE_MAX_WAITING", None),
                ("QUEUE_WAIT_TIMEOUT_MS", None),
            ],
            || {
                let (capacity, _, max_waiting, _) = resolve_queue_env(7, 1000).unwrap();
                assert_eq!(
                    php_backlog_over_connection_budget(7, capacity, max_waiting, 1000),
                    Some(1403)
                );
                // Raising `MAX_CONNECTIONS` on its own does not clear it while
                // the waiting set is left at its default, because that default
                // is half of `MAX_CONNECTIONS` and rises with it. This is why
                // the warning does not tell an operator to just raise the one
                // knob.
                let (capacity, _, max_waiting, _) = resolve_queue_env(7, 1500).unwrap();
                assert_eq!(max_waiting, 750);
                assert_eq!(
                    php_backlog_over_connection_budget(7, capacity, max_waiting, 1500),
                    Some(1653)
                );
                // A small pool on the stock budget is clear of it. A large one
                // is not: at 40 workers the queue and waiting defaults come to
                // 5120 and 5000, and with the pool itself that is 10 160
                // against 10 000 — the warning fires on a configuration nobody
                // edited, correctly by its own model. An auto-sized pool
                // reaches it from 39 workers up.
                let (capacity, _, max_waiting, _) = resolve_queue_env(7, 10_000).unwrap();
                assert_eq!(
                    php_backlog_over_connection_budget(7, capacity, max_waiting, 10_000),
                    None
                );
                let (capacity, _, max_waiting, _) = resolve_queue_env(40, 10_000).unwrap();
                assert_eq!(
                    php_backlog_over_connection_budget(40, capacity, max_waiting, 10_000),
                    Some(10_160)
                );
            },
        );

        // And through an explicit setting, which the `MAX_CONNECTIONS / 2`
        // ceiling never touches — it applies to the computed default only.
        test_env::with_env(
            &[
                ("QUEUE_CAPACITY", None),
                ("QUEUE_MAX_WAITING", Some("20000")),
            ],
            || {
                let (capacity, _, max_waiting, _) = resolve_queue_env(7, 10_000).unwrap();
                assert_eq!(max_waiting, 20_000);
                assert_eq!(
                    php_backlog_over_connection_budget(7, capacity, max_waiting, 10_000),
                    Some(20_903)
                );
            },
        );
    }

    #[test]
    fn queue_knobs_reach_the_config_payload() {
        // `to_json()` backs the `/config` endpoint; a field missing there is
        // invisible until an operator goes looking for it during an incident.
        let json = Config::test_minimal().to_json();
        assert_eq!(json["queue_capacity"], 128);
        assert_eq!(json["queue_wait_timeout_ms"], 1000);
        assert_eq!(json["queue_max_waiting"], 128);
    }

    #[test]
    fn whitespace_only_tls_cert_stays_set() {
        // `" "` is never a valid certificate path. Collapsing it to unset
        // would silently downgrade an intended-HTTPS port to plain HTTP;
        // kept as `Some` it fails closed downstream (unreadable path or
        // half-configured-pair abort).
        test_env::with_env(&[("TLS_CERT", Some(" "))], || {
            assert_eq!(
                optional_utf8_env("TLS_CERT").unwrap(),
                Some(" ".to_string())
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn tls_cert_non_utf8_is_a_hard_error() {
        use std::os::unix::ffi::OsStrExt;
        // A corrupted TLS_CERT must not silently start the server without TLS.
        let bad = std::ffi::OsStr::from_bytes(b"/etc/ssl/\xFF.pem");
        test_env::with_env_os(&[("TLS_CERT", Some(bad))], || {
            let err = optional_utf8_env("TLS_CERT").unwrap_err();
            assert!(err.to_string().contains("TLS_CERT"), "err: {err}");
        });
    }

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
    use tempfile::TempDir;

    fn with_env<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
        crate::config::test_env::with_env(vars, f);
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
