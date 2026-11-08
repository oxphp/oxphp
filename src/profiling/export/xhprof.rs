//! XHProf JSON exporter: the flat pair map `xhprof_disable()` returns.

use serde_json::{json, Map, Value};
use std::collections::HashMap;

use crate::profiling::SpanTree;

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

/// Render the tree as XHProf JSON: the `{"caller==>callee": {ct,wt,cpu,mu,pmu}}`
/// object plus the synthetic `main()` self-entry. Returns UTF-8 bytes (compact
/// JSON, no trailing newline).
pub fn export_xhprof(tree: &SpanTree) -> Vec<u8> {
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

    // Synthetic main() self-entry: aggregates all root-level spans, so a
    // viewer's root row has a value.
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
    serde_json::to_vec(&Value::Object(profile)).expect("xhprof JSON serialisation should not fail")
}
