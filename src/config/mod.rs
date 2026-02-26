mod server;

use std::fmt;
use std::path::PathBuf;

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
    pub compression: bool,
    pub access_log: AccessLogLevel,
    pub max_query_body: usize,
    /// Worker mode: PHP file that boots the application and calls oxphp_worker().
    pub worker_file: Option<PathBuf>,
    /// Max requests before recycling a worker (0 = unlimited).
    pub worker_max_requests: u64,
    /// Max memory (MB) before recycling a worker (0 = unlimited).
    pub worker_max_memory_mb: u64,
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
        let compression = std::env::var("COMPRESSION")
            .map(|v| v != "false" && v != "0" && v != "off")
            .unwrap_or(true);
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
        let worker_max_memory_mb = std::env::var("WORKER_MAX_MEMORY")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(0);

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
            compression,
            access_log,
            max_query_body,
            worker_file,
            worker_max_requests,
            worker_max_memory_mb,
        })
    }

    /// Serialize configuration to JSON for the `/config` internal endpoint.
    /// Sensitive values (TLS paths) are redacted.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "listen_addr": self.server.listen_addr,
            "document_root": self.server.document_root.display().to_string(),
            "index_file": self.server.index_file,
            "executor_type": self.executor_type,
            "max_connections": self.max_connections,
            "drain_timeout_seconds": self.drain_timeout_seconds,
            "header_timeout_seconds": self.server.header_read_timeout.as_secs(),
            "idle_timeout_seconds": self.server.idle_timeout.as_secs(),
            "request_timeout_seconds": self.server.request_timeout.as_secs(),
            "rate_limit": self.rate_limit,
            "rate_window_seconds": self.rate_window_seconds,
            "tls_enabled": self.tls_cert.is_some() && self.tls_key.is_some(),
            "error_pages_dir": self.error_pages_dir,
            "compression": self.compression,
            "access_log": self.access_log.to_string(),
            "max_query_body": self.max_query_body,
            "worker_mode": self.worker_file.is_some(),
            "worker_file": self.worker_file.as_ref().map(|p| p.display().to_string()),
            "worker_max_requests": self.worker_max_requests,
            "worker_max_memory_mb": self.worker_max_memory_mb,
        })
    }
}
