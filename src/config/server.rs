use std::path::PathBuf;
use std::time::Duration;

/// Server-specific configuration loaded from environment variables.
#[derive(Debug)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub document_root: PathBuf,
    pub header_read_timeout: Duration,
    pub request_timeout: Duration,
}

impl ServerConfig {
    pub fn new(listen_addr: String, document_root: PathBuf) -> Self {
        Self {
            listen_addr,
            document_root,
            header_read_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(120),
        }
    }

    pub fn from_env() -> Result<Self, crate::types::BoxError> {
        let listen_addr = std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:80".to_string());
        let document_root: PathBuf = std::env::var("DOCUMENT_ROOT")
            .unwrap_or_else(|_| "/var/www/html/public".to_string())
            .into();

        let header_read_timeout = Duration::from_secs(
            std::env::var("HEADER_TIMEOUT_SECONDS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(5),
        );
        let request_timeout_seconds = std::env::var("REQUEST_TIMEOUT_SECONDS")
            .ok()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(120);
        let request_timeout = if request_timeout_seconds == 0 {
            Duration::ZERO
        } else {
            Duration::from_secs(request_timeout_seconds)
        };

        Ok(Self {
            listen_addr,
            document_root,
            header_read_timeout,
            request_timeout,
        })
    }
}
