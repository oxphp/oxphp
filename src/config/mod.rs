mod server;

pub use server::ServerConfig;

/// Top-level application configuration.
#[derive(Debug)]
pub struct Config {
    pub server: ServerConfig,
    pub log_level: String,
    pub worker_threads: usize,
    pub executor_type: String,
    pub max_connections: usize,
    pub drain_timeout_secs: u64,
    pub internal_addr: Option<String>,
    pub rate_limit: u32,
    pub rate_window: u64,
    pub tls_cert: Option<String>,
    pub tls_key: Option<String>,
    pub error_pages_dir: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self, crate::types::BoxError> {
        let server = ServerConfig::from_env()?;
        let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let worker_threads = parse_worker_threads();
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

        Ok(Self {
            server,
            log_level,
            worker_threads,
            executor_type,
            max_connections,
            drain_timeout_secs,
            internal_addr,
            rate_limit,
            rate_window,
            tls_cert,
            tls_key,
            error_pages_dir,
        })
    }

    /// Serialize configuration to JSON for the `/config` internal endpoint.
    /// Sensitive values (TLS paths) are redacted.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "listen_addr": self.server.listen_addr,
            "document_root": self.server.document_root.display().to_string(),
            "index_file": self.server.index_file,
            "worker_threads": self.worker_threads,
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
        })
    }
}

/// Parse `WORKER_THREADS` env var and clamp to `cpu_count * 4`.
fn parse_worker_threads() -> usize {
    let raw = std::env::var("WORKER_THREADS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0);

    let cpu_count = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let max_threads = cpu_count * 4;

    if raw > max_threads {
        max_threads
    } else {
        raw
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn test_worker_threads_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::remove_var("WORKER_THREADS");
        assert_eq!(parse_worker_threads(), 0);
    }

    #[test]
    fn test_worker_threads_explicit() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("WORKER_THREADS", "2");
        assert_eq!(parse_worker_threads(), 2);
        std::env::remove_var("WORKER_THREADS");
    }

    #[test]
    fn test_worker_threads_clamped() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("WORKER_THREADS", "999999");
        let result = parse_worker_threads();
        let max = std::thread::available_parallelism()
            .map(|n| n.get() * 4)
            .unwrap_or(4);
        assert_eq!(result, max);
        std::env::remove_var("WORKER_THREADS");
    }

    #[test]
    fn test_worker_threads_invalid() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("WORKER_THREADS", "abc");
        assert_eq!(parse_worker_threads(), 0);
        std::env::remove_var("WORKER_THREADS");
    }
}
