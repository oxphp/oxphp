//! Storage subsystem for captured profiles.
//!
//! Contains:
//! - `RunMeta` — per-run context object built at complete time.
//! - `ProfileCache` — in-memory LRU keyed by `run_id`.
//! - `DiskWriter` — fan-out 4 format files + index.json.
//! - `RetentionTask` — background trim of oldest entries.
//! - `HttpPusher` — single-URL push with retry+backoff.
//! - `Storage` — composite handle owned by `ProfilerPlugin` and
//!   passed to `ProfilerCompleteHandler` for the fan-out.

pub mod cache;
pub mod disk;
pub mod http;
pub mod meta;
pub mod metrics;
pub mod retention;

pub use cache::ProfileCache;
pub use disk::{DiskWriter, OutputFormat, TokenBucket};
pub use http::HttpPusher;
pub use meta::RunMeta;
pub use metrics::StorageMetrics;

use std::sync::Arc;

/// The composite storage handle. Disk + HTTP optional; cache
/// always present. Built once at `ProfilerPlugin::init`, cloned
/// (Arc) into `ProfilerCompleteHandler` for the per-run fan-out.
pub struct Storage {
    pub cache: Arc<ProfileCache>,
    pub disk: Option<Arc<DiskWriter>>,
    pub http: Option<Arc<HttpPusher>>,
    pub metrics: Arc<StorageMetrics>,
}
