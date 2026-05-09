use std::path::PathBuf;
use std::time::Duration;

/// Server-specific configuration loaded from environment variables.
#[derive(Debug)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub document_root: PathBuf,
    pub header_read_timeout: Duration,
}

impl ServerConfig {
    pub fn new(listen_addr: String, document_root: PathBuf) -> Self {
        Self {
            listen_addr,
            document_root,
            header_read_timeout: Duration::from_secs(5),
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

        Ok(Self {
            listen_addr,
            document_root,
            header_read_timeout,
        })
    }
}
