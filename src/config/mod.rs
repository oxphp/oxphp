mod proxy;
mod server;

use std::fmt;
use std::path::PathBuf;

pub use proxy::TrustedProxyConfig;
pub use server::ServerConfig;

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
    pub rate_limit: u32,
    pub rate_window_seconds: u64,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub error_pages_dir: Option<String>,
    pub compression_level: i32,
    pub access_log: AccessLogLevel,
    pub max_query_body: usize,
    /// Worker mode: PHP file that boots the application and calls oxphp_worker().
    pub worker_file: Option<PathBuf>,
    /// Max requests before recycling a worker (0 = unlimited).
    pub worker_max_requests: u64,
    /// Max memory (MB) before recycling a worker (0 = unlimited).
    pub worker_max_memory_mib: u64,
    /// Static file cache TTL in seconds. `None` = caching disabled.
    pub static_cache_ttl: Option<u64>,
    /// Whether cached content is served without mtime revalidation.
    /// When `true` (default), cached entries are returned immediately.
    /// When `false` (`STATIC_CACHE=off`), each hit performs a `stat()` check
    /// and evicts stale entries before serving.
    pub static_cache_enabled: bool,
    /// Number of dedicated async worker threads. 0 = async pool disabled.
    pub async_workers: usize,
    /// Bounded channel capacity for pending async tasks. 0 = auto (async_workers * 64).
    pub async_queue_capacity: usize,
    /// W3C Trace Context propagation enabled.
    pub trace_context: bool,
    /// Whether PHP superglobals ($_GET, $_POST, etc.) are populated.
    /// When false, only the object API (oxphp_http_request()) provides request data.
    pub superglobals_enabled: bool,
    /// PHP worker pool description (e.g. "4", "2:8", "4 (auto)").
    pub php_workers: String,
    /// Effective number of Tokio runtime threads.
    pub tokio_workers: usize,
    /// Bounded channel capacity for PHP request queue.
    pub queue_capacity: usize,
    /// Trusted reverse proxy networks (CIDR). When set, X-Forwarded-* and
    /// Forwarded headers from these peers are trusted for client IP extraction.
    pub trusted_proxies: Option<TrustedProxyConfig>,
}

/// Parse a duration string like "30s", "5m", "2h", "30d", "1w", "1y", "3600", or "off".
/// Bare numbers (e.g. "3600") are treated as seconds.
/// Returns `None` for "off" or invalid input.
pub fn parse_duration(s: &str) -> Option<u64> {
    let s = s.trim();
    if s.eq_ignore_ascii_case("off") {
        return None;
    }
    if s.is_empty() {
        return None;
    }
    // Bare number = seconds
    if let Ok(secs) = s.parse::<u64>() {
        return Some(secs);
    }
    let (num_str, suffix) = s.split_at(s.len() - 1);
    let num: u64 = num_str.parse().ok()?;
    let multiplier = match suffix {
        "s" => 1,
        "m" => 60,
        "h" => 3_600,
        "d" => 86_400,
        "w" => 604_800,
        "y" => 31_536_000,
        _ => return None,
    };
    Some(num.saturating_mul(multiplier))
}

impl Config {
    pub fn from_env() -> Result<Self, crate::types::BoxError> {
        let server = ServerConfig::from_env()?;
        let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let executor_type = std::env::var("EXECUTOR").unwrap_or_else(|_| "sapi".to_string());
        let max_connections = std::env::var("MAX_CONNECTIONS")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(10_000);
        let drain_timeout_seconds = std::env::var("DRAIN_TIMEOUT_SECONDS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(30);
        let internal_addr = std::env::var("INTERNAL_ADDR").ok();
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
                    eprintln!(
                        "Error: COMPRESSION_LEVEL must be 0-11 (got {:?}), 0 = disabled",
                        val
                    );
                    std::process::exit(1);
                }
            },
            Err(_) => 4,
        };
        let access_log = match std::env::var("ACCESS_LOG").as_deref() {
            Ok("all") => AccessLogLevel::All,
            Ok("error") => AccessLogLevel::Error,
            Ok("") | Err(_) => AccessLogLevel::Off,
            Ok(other) => {
                eprintln!(
                    "Warning: unknown ACCESS_LOG value {:?}, expected \"all\", \"error\", or empty — defaulting to off",
                    other
                );
                AccessLogLevel::Off
            }
        };
        let max_query_body = std::env::var("MAX_QUERY_BODY")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(512 * 1024);

        let worker_file = std::env::var("WORKER_FILE")
            .ok()
            .filter(|s| !s.is_empty())
            .map(PathBuf::from);
        let worker_max_requests = std::env::var("WORKER_MAX_REQUESTS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);
        let worker_max_memory_mib = std::env::var("WORKER_MAX_MEMORY_MIB")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

        let static_cache_ttl = match std::env::var("STATIC_CACHE_TTL") {
            Ok(val) => parse_duration(&val),
            Err(_) => Some(2_592_000), // 30 days
        };

        let static_cache_enabled = std::env::var("STATIC_CACHE")
            .map(|v| !v.eq_ignore_ascii_case("off"))
            .unwrap_or(true);

        let async_workers: usize = std::env::var("ASYNC_WORKERS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let async_queue_capacity: usize = std::env::var("ASYNC_QUEUE_CAPACITY")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);

        let trace_context = std::env::var("TRACE_CONTEXT")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

        let superglobals_enabled = std::env::var("SUPERGLOBALS_ENABLED")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        let cpu = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        let default_workers = (cpu / 2).max(1);

        let (php_workers, php_worker_count) = match std::env::var("PHP_WORKERS") {
            Ok(val) if !val.is_empty() => {
                // Parse worker count for queue_capacity default.
                let count = if let Some((min_s, _)) = val.split_once(':') {
                    min_s.parse::<usize>().unwrap_or(default_workers)
                } else {
                    val.parse::<usize>().unwrap_or(default_workers)
                };
                (val, count)
            }
            _ => (format!("{default_workers} (auto)"), default_workers),
        };

        let tokio_workers = std::env::var("TOKIO_WORKERS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(default_workers);

        let queue_capacity = std::env::var("QUEUE_CAPACITY")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(php_worker_count * 128);

        let trusted_proxies = match TrustedProxyConfig::from_env() {
            Ok(tp) => tp,
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        };

        Ok(Self {
            server,
            log_level,
            executor_type,
            max_connections,
            drain_timeout_seconds,
            internal_addr,
            rate_limit,
            rate_window_seconds,
            tls_cert,
            tls_key,
            error_pages_dir,
            compression_level,
            access_log,
            max_query_body,
            worker_file,
            worker_max_requests,
            worker_max_memory_mib,
            static_cache_ttl,
            static_cache_enabled,
            async_workers,
            async_queue_capacity,
            trace_context,
            superglobals_enabled,
            php_workers,
            tokio_workers,
            queue_capacity,
            trusted_proxies,
        })
    }

    /// Serialize configuration to JSON for the `/config` internal endpoint.
    /// Sensitive values (TLS paths) are redacted.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "listen_addr": self.server.listen_addr,
            "document_root": self.server.document_root.display().to_string(),
            "index_file": self.server.index_file,
            "log_level": self.log_level,
            "executor_type": self.executor_type,
            "php_workers": self.php_workers,
            "tokio_workers": self.tokio_workers,
            "queue_capacity": self.queue_capacity,
            "max_connections": self.max_connections,
            "drain_timeout_seconds": self.drain_timeout_seconds,
            "internal_addr": self.internal_addr,
            "header_timeout_seconds": self.server.header_read_timeout.as_secs(),
            "request_timeout_seconds": self.server.request_timeout.as_secs(),
            "rate_limit": self.rate_limit,
            "rate_window_seconds": self.rate_window_seconds,
            "tls_enabled": self.tls_cert.is_some() && self.tls_key.is_some(),
            "error_pages_dir": self.error_pages_dir,
            "compression_level": self.compression_level,
            "access_log": self.access_log.to_string(),
            "max_query_body": self.max_query_body,
            "worker_mode": self.worker_file.is_some(),
            "worker_file": self.worker_file.as_ref().map(|p| p.display().to_string()),
            "worker_max_requests": self.worker_max_requests,
            "worker_max_memory_mib": self.worker_max_memory_mib,
            "static_cache_ttl": self.static_cache_ttl,
            "static_cache_enabled": self.static_cache_enabled,
            "async_workers": self.async_workers,
            "async_queue_capacity": if self.async_queue_capacity > 0 {
                self.async_queue_capacity
            } else {
                self.async_workers * 64
            },
            "trace_context": self.trace_context,
            "superglobals_enabled": self.superglobals_enabled,
            "split_path_info": self.server.split_path_info,
            "trusted_proxies": self.trusted_proxies.is_some(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_seconds() {
        assert_eq!(parse_duration("30s"), Some(30));
        assert_eq!(parse_duration("0s"), Some(0));
        assert_eq!(parse_duration("1s"), Some(1));
    }

    #[test]
    fn test_parse_duration_minutes() {
        assert_eq!(parse_duration("5m"), Some(300));
        assert_eq!(parse_duration("1m"), Some(60));
    }

    #[test]
    fn test_parse_duration_hours() {
        assert_eq!(parse_duration("1h"), Some(3600));
        assert_eq!(parse_duration("24h"), Some(86400));
    }

    #[test]
    fn test_parse_duration_days() {
        assert_eq!(parse_duration("30d"), Some(2_592_000));
        assert_eq!(parse_duration("1d"), Some(86400));
    }

    #[test]
    fn test_parse_duration_weeks() {
        assert_eq!(parse_duration("1w"), Some(604_800));
        assert_eq!(parse_duration("2w"), Some(1_209_600));
    }

    #[test]
    fn test_parse_duration_years() {
        assert_eq!(parse_duration("1y"), Some(31_536_000));
    }

    #[test]
    fn test_parse_duration_off() {
        assert_eq!(parse_duration("off"), None);
        assert_eq!(parse_duration("OFF"), None);
        assert_eq!(parse_duration("Off"), None);
    }

    #[test]
    fn test_parse_duration_bare_number() {
        assert_eq!(parse_duration("3600"), Some(3600));
        assert_eq!(parse_duration("0"), Some(0));
        assert_eq!(parse_duration("30"), Some(30));
    }

    #[test]
    fn test_parse_duration_invalid() {
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("abc"), None);
        assert_eq!(parse_duration("30x"), None);
    }

    #[test]
    fn test_parse_duration_whitespace() {
        assert_eq!(parse_duration("  30s  "), Some(30));
        assert_eq!(parse_duration(" off "), None);
    }

    #[test]
    fn test_static_cache_parse_off() {
        let enabled = Some("off")
            .map(|v: &str| !v.eq_ignore_ascii_case("off"))
            .unwrap_or(true);
        assert!(!enabled);
    }

    #[test]
    fn test_static_cache_parse_off_uppercase() {
        let enabled = Some("OFF")
            .map(|v: &str| !v.eq_ignore_ascii_case("off"))
            .unwrap_or(true);
        assert!(!enabled);
    }

    #[test]
    fn test_static_cache_parse_default() {
        let enabled: Option<&str> = None;
        let result = enabled
            .map(|v| !v.eq_ignore_ascii_case("off"))
            .unwrap_or(true);
        assert!(result);
    }

    #[test]
    fn test_static_cache_parse_on() {
        let enabled = Some("on")
            .map(|v: &str| !v.eq_ignore_ascii_case("off"))
            .unwrap_or(true);
        assert!(enabled);
    }
}
