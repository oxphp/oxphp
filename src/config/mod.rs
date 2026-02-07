mod server;

pub use server::ServerConfig;

/// Top-level application configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub log_level: String,
}

impl Config {
    pub fn from_env() -> Result<Self, crate::types::BoxError> {
        let server = ServerConfig::from_env()?;
        let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        Ok(Self { server, log_level })
    }
}
