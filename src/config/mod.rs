mod server;

pub use server::ServerConfig;

/// Top-level application configuration.
#[derive(Debug)]
pub struct Config {
    pub server: ServerConfig,
    pub log_level: String,
    pub executor_type: String,
    pub max_connections: usize,
    pub drain_timeout_secs: u64,
    pub internal_addr: Option<String>,
    pub rate_limit: u32,
    pub rate_window: u64,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub error_pages_dir: Option<String>,
    pub compression: bool,
    pub access_log: bool,
    pub max_query_body: usize,
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
        let drain_timeout_secs = std::env::var("DRAIN_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(30);
        let internal_addr = std::env::var("INTERNAL_ADDR").ok();
        let rate_limit = std::env::var("RATE_LIMIT")
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
            .unwrap_or(0);
        let rate_window = std::env::var("RATE_WINDOW")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(60);
        let tls_cert = std::env::var("TLS_CERT").ok();
        let tls_key = std::env::var("TLS_KEY").ok();
        let error_pages_dir = std::env::var("ERROR_PAGES_DIR").ok();
        let compression = std::env::var("COMPRESSION")
            .map(|v| v != "false" && v != "0" && v != "off")
            .unwrap_or(true);
        let access_log = std::env::var("ACCESS_LOG")
            .map(|v| v != "false" && v != "0" && v != "off")
            .unwrap_or(true);
        let max_query_body = std::env::var("MAX_QUERY_BODY")
            .ok()
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(512 * 1024);

        Ok(Self {
            server,
            log_level,
            executor_type,
            max_connections,
            drain_timeout_secs,
            internal_addr,
            rate_limit,
            rate_window,
            tls_cert,
            tls_key,
            error_pages_dir,
            compression,
            access_log,
            max_query_body,
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
            "drain_timeout_secs": self.drain_timeout_secs,
            "header_timeout_secs": self.server.header_read_timeout.as_secs(),
            "idle_timeout_secs": self.server.idle_timeout.as_secs(),
            "request_timeout_secs": self.server.request_timeout.as_secs(),
            "rate_limit": self.rate_limit,
            "rate_window": self.rate_window,
            "tls_enabled": self.tls_cert.is_some() && self.tls_key.is_some(),
            "error_pages_dir": self.error_pages_dir,
            "compression": self.compression,
            "access_log": self.access_log,
            "max_query_body": self.max_query_body,
        })
    }
}
