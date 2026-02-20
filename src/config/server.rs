use std::path::PathBuf;
use std::time::Duration;

/// Server-specific configuration loaded from environment variables.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub document_root: PathBuf,
    pub index_file: Option<String>,
    pub header_read_timeout: Duration,
    pub idle_timeout: Duration,
    pub request_timeout: Duration,
}

impl ServerConfig {
    pub fn new(listen_addr: String, document_root: PathBuf, index_file: Option<String>) -> Self {
        Self {
            listen_addr,
            document_root,
            index_file,
            header_read_timeout: Duration::from_secs(5),
            idle_timeout: Duration::from_secs(60),
            request_timeout: Duration::from_secs(120),
        }
    }

    pub fn from_env() -> Result<Self, crate::types::BoxError> {
        let listen_addr =
            std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let document_root: PathBuf = std::env::var("DOCUMENT_ROOT")
            .unwrap_or_else(|_| "/var/www/html".to_string())
            .into();
        let index_file = std::env::var("INDEX_FILE").ok();

        let header_read_timeout = Duration::from_secs(
            std::env::var("HEADER_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(5),
        );
        let idle_timeout = Duration::from_secs(
            std::env::var("IDLE_TIMEOUT_SECS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(60),
        );
        let request_timeout_secs = std::env::var("REQUEST_TIMEOUT_SECS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(120);
        let request_timeout = if request_timeout_secs == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs(request_timeout_secs)
        };

        Ok(Self {
            listen_addr,
            document_root,
            index_file,
            header_read_timeout,
            idle_timeout,
            request_timeout,
        })
    }
}
