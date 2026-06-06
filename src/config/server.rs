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

/// HTTP/2 connection-level limits. All fields map 1-to-1 to hyper's
/// `Http2` builder methods and are applied once per server instance.
#[derive(Clone, Debug)]
pub struct H2Config {
    /// Max simultaneous open streams per connection. Default: `max_worker_count * 4`
    /// (floor 32). Tuned for a blocking worker-pool backend where each open stream
    /// directly maps to a queued PHP request — one connection cannot hold more
    /// streams than this floor, bounding single-source queue amplification.
    ///
    /// Note: this is intentionally ≈ pool capacity, not a browser-multiplex target.
    /// Browsers respect SETTINGS and retry REFUSED_STREAM automatically, so raising
    /// this beyond the worker count only increases queue pressure per connection.
    pub max_concurrent_streams: u32,
    /// Max RST_STREAM frames queued before the connection is closed. Explicit value
    /// keeps the Rapid Reset (CVE-2023-44487) defence documented and operator-visible.
    pub max_pending_accept_reset: usize,
    /// Max total decoded bytes across all headers in one request (HPACK bomb guard).
    pub max_header_list_bytes: u32,
    /// PING keepalive interval. `None` disables keepalive.
    pub keepalive_interval: Option<Duration>,
    /// How long to wait for a PING reply before closing the connection.
    pub keepalive_timeout: Duration,
}

impl Default for H2Config {
    /// Returns hardcoded floor defaults (equivalent to `from_env(1)` with no env set).
    /// Use in tests and anywhere a concrete default is needed without reading env.
    fn default() -> Self {
        Self {
            max_concurrent_streams: 32,
            max_pending_accept_reset: 20,
            max_header_list_bytes: 64 * 1024,
            keepalive_interval: Some(Duration::from_secs(20)),
            keepalive_timeout: Duration::from_secs(10),
        }
    }
}

impl H2Config {
    /// Build from environment variables; `max_worker_count` drives the default
    /// for `max_concurrent_streams` when `H2_MAX_CONCURRENT_STREAMS` is not set.
    pub fn from_env(max_worker_count: usize) -> Self {
        let default_streams = (max_worker_count * 4).max(32) as u32;

        let max_concurrent_streams = std::env::var("H2_MAX_CONCURRENT_STREAMS")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(default_streams);

        let max_pending_accept_reset = std::env::var("H2_MAX_PENDING_RESET")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(20); // h2 crate DEFAULT_REMOTE_RESET_STREAM_MAX

        let max_header_list_bytes = std::env::var("H2_MAX_HEADER_LIST_BYTES")
            .ok()
            .and_then(|v| v.parse::<u32>().ok())
            .unwrap_or(64 * 1024);

        let keepalive_interval_secs = std::env::var("H2_KEEPALIVE_INTERVAL_SECS")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(20);
        let keepalive_interval = if keepalive_interval_secs == 0 {
            None
        } else {
            Some(Duration::from_secs(keepalive_interval_secs))
        };

        let keepalive_timeout = Duration::from_secs(
            std::env::var("H2_KEEPALIVE_TIMEOUT_SECS")
                .ok()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(10)
                .max(1), // 0 would make every PING fail immediately
        );

        Self {
            max_concurrent_streams,
            max_pending_accept_reset,
            max_header_list_bytes,
            keepalive_interval,
            keepalive_timeout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Env vars are process-global; this lock serializes all env-touching tests.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    const H2_VARS: &[&str] = &[
        "H2_MAX_CONCURRENT_STREAMS",
        "H2_MAX_PENDING_RESET",
        "H2_MAX_HEADER_LIST_BYTES",
        "H2_KEEPALIVE_INTERVAL_SECS",
        "H2_KEEPALIVE_TIMEOUT_SECS",
    ];

    /// Lock + set vars + run + restore. Also snapshots any pre-existing values
    /// so a CI environment with H2_* already set does not pollute other tests.
    fn with_env<F: FnOnce()>(vars: &[(&str, &str)], f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved: Vec<(&str, Option<String>)> =
            vars.iter().map(|(k, _)| (*k, std::env::var(k).ok())).collect();
        for (k, v) in vars {
            std::env::set_var(k, v);
        }
        f();
        for (k, orig) in &saved {
            match orig {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }

    /// Lock + clear all H2_* vars + run + restore. Guarantees defaults even when
    /// CI sets H2_* in the environment.
    fn without_h2_env<F: FnOnce()>(f: F) {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let saved: Vec<(&str, Option<String>)> =
            H2_VARS.iter().map(|k| (*k, std::env::var(k).ok())).collect();
        for k in H2_VARS {
            std::env::remove_var(k);
        }
        f();
        for (k, orig) in &saved {
            match orig {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }

    #[test]
    fn h2_defaults_are_worker_aware() {
        without_h2_env(|| {
            let cfg = H2Config::from_env(8);
            // 8 workers * 4 = 32, exactly at the floor
            assert_eq!(cfg.max_concurrent_streams, 32);
            assert_eq!(cfg.max_pending_accept_reset, 20);
            assert_eq!(cfg.max_header_list_bytes, 65536);
            assert_eq!(cfg.keepalive_interval, Some(Duration::from_secs(20)));
            assert_eq!(cfg.keepalive_timeout, Duration::from_secs(10));
        });
    }

    #[test]
    fn h2_minimum_concurrent_streams() {
        without_h2_env(|| {
            // Even with 1 worker, floor at 32
            let cfg = H2Config::from_env(1);
            assert_eq!(cfg.max_concurrent_streams, 32);
        });
    }

    #[test]
    fn h2_above_floor() {
        without_h2_env(|| {
            // 9 workers * 4 = 36 > floor 32
            let cfg = H2Config::from_env(9);
            assert_eq!(cfg.max_concurrent_streams, 36);
        });
    }

    #[test]
    fn h2_env_overrides() {
        with_env(
            &[
                ("H2_MAX_CONCURRENT_STREAMS", "50"),
                ("H2_MAX_PENDING_RESET", "40"),
                ("H2_MAX_HEADER_LIST_BYTES", "32768"),
                ("H2_KEEPALIVE_INTERVAL_SECS", "30"),
                ("H2_KEEPALIVE_TIMEOUT_SECS", "15"),
            ],
            || {
                let cfg = H2Config::from_env(8);
                assert_eq!(cfg.max_concurrent_streams, 50);
                assert_eq!(cfg.max_pending_accept_reset, 40);
                assert_eq!(cfg.max_header_list_bytes, 32768);
                assert_eq!(cfg.keepalive_interval, Some(Duration::from_secs(30)));
                assert_eq!(cfg.keepalive_timeout, Duration::from_secs(15));
            },
        );
    }

    #[test]
    fn h2_keepalive_interval_zero_disables() {
        with_env(&[("H2_KEEPALIVE_INTERVAL_SECS", "0")], || {
            let cfg = H2Config::from_env(8);
            assert_eq!(cfg.keepalive_interval, None);
        });
    }

    #[test]
    fn h2_keepalive_timeout_zero_clamped_to_one() {
        with_env(&[("H2_KEEPALIVE_TIMEOUT_SECS", "0")], || {
            let cfg = H2Config::from_env(8);
            assert_eq!(cfg.keepalive_timeout, Duration::from_secs(1));
        });
    }

    #[test]
    fn h2_invalid_env_falls_back_to_default() {
        with_env(&[("H2_MAX_CONCURRENT_STREAMS", "not_a_number")], || {
            let cfg = H2Config::from_env(8);
            // 8 workers * 4 = 32
            assert_eq!(cfg.max_concurrent_streams, 32);
        });
    }

    #[test]
    fn h2_default_impl_matches_floor() {
        let d = H2Config::default();
        assert_eq!(d.max_concurrent_streams, 32);
        assert_eq!(d.max_pending_accept_reset, 20);
        assert_eq!(d.max_header_list_bytes, 64 * 1024);
        assert_eq!(d.keepalive_interval, Some(Duration::from_secs(20)));
        assert_eq!(d.keepalive_timeout, Duration::from_secs(10));
    }
}
