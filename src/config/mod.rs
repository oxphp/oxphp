mod server;

pub use server::ServerConfig;

/// Top-level application configuration.
#[derive(Debug, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub log_level: String,
    pub worker_threads: usize,
}

impl Config {
    pub fn from_env() -> Result<Self, crate::types::BoxError> {
        let server = ServerConfig::from_env()?;
        let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
        let worker_threads = parse_worker_threads();

        Ok(Self {
            server,
            log_level,
            worker_threads,
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
