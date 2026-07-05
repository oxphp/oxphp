//! Per-run metadata captured at storage time. Centralises the
//! request-context fields used by:
//! - `index.json` per-line entry (disk.rs)
//! - xhgui envelope (built via `meta.to_xhgui_meta()`)
//! - pprof labels and time_nanos
//! - Internal routes + Prometheus metric labels
//!
//! Built once at `ProfilerCompleteHandler::handle` from the
//! `PluginCompleteView` and the `Arc<SpanTree>`.

use serde::{Deserialize, Serialize};

use crate::plugins::ox_profiler::trigger::ActivationSource;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunMeta {
    pub run_id: String,
    pub request_id: String,
    pub trace_id: Option<String>,
    /// Unix epoch milliseconds at storage time.
    pub timestamp_ms: u64,
    /// Wall-clock duration of the profiled request, milliseconds.
    pub duration_ms: u32,
    pub method: String,
    pub url: String,
    pub status: u16,
    pub user_agent: Option<String>,
    pub client_ip: Option<String>,
    pub source: ActivationSource,
    pub span_count: u32,
    pub event_count: u32,
    pub error_count: u16,
    pub leaked_count: u16,
    pub truncated: bool,
    pub oxphp_version: String,
    /// Set of formats this run was actually written in (the index
    /// entry uses this; storage may write a subset of the
    /// configured formats if some writes failed).
    pub formats: Vec<String>,
}

impl RunMeta {
    /// Storage time (request completion) as whole Unix epoch seconds — this is
    /// `timestamp_ms`, which `build_run_meta` stamps at request-complete, not
    /// request start. Shared by the xhgui `meta` and the Buggregator envelope's
    /// `date` so their time mapping can't drift.
    pub fn timestamp_secs(&self) -> u64 {
        self.timestamp_ms / 1000
    }

    /// Build the xhgui envelope's `meta` struct from this RunMeta.
    /// Only the fields xhgui actually inspects are populated.
    pub fn to_xhgui_meta(&self) -> crate::profiling::export::XhguiMeta {
        crate::profiling::export::XhguiMeta {
            url: self.url.clone(),
            request_method: self.method.clone(),
            request_ts: self.timestamp_secs(),
            request_ts_micro: (self.timestamp_ms as f64) / 1000.0,
            server_name: String::new(),
            get: serde_json::Map::new(),
            post: serde_json::Map::new(),
            server: serde_json::Map::new(),
        }
    }
}
