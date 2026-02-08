use std::path::PathBuf;

/// Server-specific configuration loaded from environment variables.
#[derive(Debug)]
pub struct ServerConfig {
    pub listen_addr: String,
    pub document_root: PathBuf,
    pub index_file: Option<String>,
}

impl ServerConfig {
    pub fn new(listen_addr: String, document_root: PathBuf, index_file: Option<String>) -> Self {
        Self {
            listen_addr,
            document_root,
            index_file,
        }
    }

    pub fn from_env() -> Result<Self, crate::types::BoxError> {
        let listen_addr =
            std::env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());
        let document_root: PathBuf = std::env::var("DOCUMENT_ROOT")
            .unwrap_or_else(|_| "/var/www/html".to_string())
            .into();
        let index_file = std::env::var("INDEX_FILE").ok();
        Ok(Self {
            listen_addr,
            document_root,
            index_file,
        })
    }
}
