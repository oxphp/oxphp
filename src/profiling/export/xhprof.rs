//! XHProf JSON exporter — flat pair map + optional xhgui envelope.
//!
//! Output is xhgui's `/run/import` import format. Compatible with
//! the upstream xhgui parser used by Tideways, Splat, and other
//! XHProf-derived UIs.

use serde_json::{json, Map, Value};
use std::collections::HashMap;

use crate::profiling::SpanTree;

/// Output mode for `export_xhprof`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XhprofMode {
    /// Bare pair-map: `{"a==>b": {ct, wt, cpu, mu, pmu}, ...}`.
    Raw,
    /// `{"profile": <pair_map>, "meta": <XhguiMeta>}` — xhgui-import
    /// compatible.
    Xhgui,
}

/// Request-context fields the xhgui envelope expects. Storage
/// fills these from the `PluginCompleteView` at export time.
#[derive(Debug, Clone, Default)]
pub struct XhguiMeta {
    /// Combined `METHOD URL` for the `simple_url` field — xhgui's
    /// primary group key. Leave empty for non-HTTP requests.
    pub url: String,
    /// HTTP method as the `request_method` field.
    pub request_method: String,
    /// Unix epoch timestamp (seconds) at request start.
    pub request_ts: u64,
    /// Same, with microsecond precision (xhgui likes both).
    pub request_ts_micro: f64,
    /// Server name / hostname for the `SERVER` envelope.
    pub server_name: String,
    /// Optional structured GET / POST / SERVER hash maps. Empty by
    /// default; storage may populate from the request view.
    pub get: Map<String, Value>,
    pub post: Map<String, Value>,
    pub server: Map<String, Value>,
}

#[derive(Debug, Default, Clone, Copy)]
struct PairAccum {
    ct: u64,
    wt: u64,
    cpu: u64,
    mu: i64,
    pmu: i64,
}

impl PairAccum {
    fn add(&mut self, span_wt: u64, span_cpu: u64, span_mu: i64, span_pmu: i64) {
        self.ct = self.ct.saturating_add(1);
        self.wt = self.wt.saturating_add(span_wt);
        self.cpu = self.cpu.saturating_add(span_cpu);
        self.mu = self.mu.saturating_add(span_mu.max(0));
        self.pmu = self.pmu.saturating_add(span_pmu.max(0));
    }
}

/// Render the tree as XHProf JSON. Returns UTF-8 bytes (compact JSON,
/// no trailing newline).
pub fn export_xhprof(tree: &SpanTree, mode: XhprofMode, meta: Option<XhguiMeta>) -> Vec<u8> {
    let mut pair_map: HashMap<String, PairAccum> = HashMap::new();

    // Build span_id → &FinishedSpan once.
    let mut by_id: HashMap<&str, &crate::profiling::FinishedSpan> =
        HashMap::with_capacity(tree.finished.len());
    for s in &tree.finished {
        by_id.insert(s.span_id.as_ref(), s);
    }

    for span in &tree.finished {
        let parent_name: &str = if span.parent_span_id == tree.root_span_id {
            "main()"
        } else {
            by_id
                .get(span.parent_span_id.as_ref())
                .map(|p| p.name.as_ref())
                .unwrap_or("main()") // orphan → re-parent to main()
        };
        let key = format!("{parent_name}==>{}", span.name);

        let wt = span.end_ns.saturating_sub(span.start_ns) / 1000;
        let cpu = span.cpu_ns / 1000;
        let mu = span.mem_exit - span.mem_enter;
        let pmu = span.mem_peak - span.mem_enter;

        pair_map.entry(key).or_default().add(wt, cpu, mu, pmu);
    }

    // Synthetic main() self-entry: aggregates all root-level spans.
    // xhgui needs this so the root row in the UI has a value.
    let mut main_self = PairAccum::default();
    for span in &tree.finished {
        if span.parent_span_id == tree.root_span_id {
            let wt = span.end_ns.saturating_sub(span.start_ns) / 1000;
            let cpu = span.cpu_ns / 1000;
            let mu = span.mem_exit - span.mem_enter;
            let pmu = span.mem_peak - span.mem_enter;
            main_self.add(wt, cpu, mu, pmu);
        }
    }
    if main_self.ct > 0 {
        pair_map.insert("main()".to_string(), main_self);
    }

    // Serialise the pair map. Sort keys for deterministic output —
    // makes golden fixtures stable.
    let mut sorted_keys: Vec<&String> = pair_map.keys().collect();
    sorted_keys.sort();
    let mut profile = Map::new();
    for k in sorted_keys {
        let p = pair_map.get(k).unwrap();
        profile.insert(
            k.clone(),
            json!({
                "ct": p.ct,
                "wt": p.wt,
                "cpu": p.cpu,
                "mu": p.mu,
                "pmu": p.pmu,
            }),
        );
    }
    let profile_value = Value::Object(profile);

    let final_value = match mode {
        XhprofMode::Raw => profile_value,
        XhprofMode::Xhgui => {
            let meta = meta.unwrap_or_default();
            let mut envelope = Map::new();
            envelope.insert("profile".into(), profile_value);
            envelope.insert(
                "meta".into(),
                json!({
                    "url": meta.url,
                    "simple_url": meta.url,
                    "request_method": meta.request_method,
                    "request_ts": meta.request_ts,
                    "request_ts_micro": meta.request_ts_micro,
                    "request_date": "",   // optional; storage may set
                    "SERVER": meta.server,
                    "GET": meta.get,
                    "POST": meta.post,
                    "server_name": meta.server_name,
                }),
            );
            Value::Object(envelope)
        }
    };

    serde_json::to_vec(&final_value).expect("xhprof JSON serialisation should not fail")
}
