//! Per-run metadata captured at storage time. Centralises the
//! request-context fields used by:
//! - `index.json` per-line entry (disk.rs)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_line_with_removed_trigger_parses_as_header() {
        for source in ["Cookie", "Query"] {
            let line = format!(
                r#"{{"run_id":"r","request_id":"q","trace_id":null,"timestamp_ms":1,"duration_ms":1,"method":"GET","url":"/","status":200,"user_agent":null,"client_ip":null,"source":"{source}","span_count":0,"event_count":0,"error_count":0,"leaked_count":0,"truncated":false,"oxphp_version":"0.15.0","formats":[]}}"#
            );
            let meta: RunMeta = serde_json::from_str(&line).expect(source);
            assert_eq!(meta.source, ActivationSource::Header, "{source}");
        }
    }
}
