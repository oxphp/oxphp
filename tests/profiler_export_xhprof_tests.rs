//! Integration tests for the XHProf JSON exporter.

#![cfg(feature = "plugin-profiler")]

use oxphp::profiling::export::{export_xhprof, XhguiMeta, XhprofMode};
use oxphp::profiling::{ProfilingContext, ProfilingMode};
use serde_json::Value;

fn build_simple_tree() -> oxphp::profiling::SpanTree {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
    // push_with_metrics: (name, attrs, start_ns, cpu_start_ns, mem_enter, mem_peak_enter)
    // pop_with_metrics:  (id,          end_ns,   cpu_end_ns,   mem_exit,  mem_peak_exit)
    let outer = ctx.push_with_metrics("outer".into(), vec![], 1_000, 0, 1000, 1500);
    let middle = ctx.push_with_metrics("middle".into(), vec![], 2_000, 100, 1100, 1600);
    ctx.pop_with_metrics(middle, 3_000, 200, 1300, 1700);
    ctx.pop_with_metrics(outer, 4_000, 250, 1400, 1900);
    ctx.finalize().as_ref().clone()
}

#[test]
fn raw_mode_emits_pair_map() {
    let tree = build_simple_tree();
    let bytes = export_xhprof(&tree, XhprofMode::Raw, None);
    let v: Value = serde_json::from_slice(&bytes).expect("valid JSON");
    let map = v.as_object().expect("top-level is an object");

    // Three entries: main()==>outer, outer==>middle, main() self.
    assert!(map.contains_key("main()==>outer"), "{map:?}");
    assert!(map.contains_key("outer==>middle"), "{map:?}");
    assert!(map.contains_key("main()"), "{map:?}");
}

#[test]
fn pair_entries_have_5_metrics() {
    let tree = build_simple_tree();
    let bytes = export_xhprof(&tree, XhprofMode::Raw, None);
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let entry = v.get("main()==>outer").unwrap().as_object().unwrap();
    for k in &["ct", "wt", "cpu", "mu", "pmu"] {
        assert!(entry.contains_key(*k), "missing {k}: {entry:?}");
    }
}

#[test]
fn aggregation_sums_repeated_parent_child_pairs() {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());

    // Three invocations of the same outer→inner pair.
    for _ in 0..3 {
        let outer = ctx.push_with_metrics("outer".into(), vec![], 0, 0, 1000, 1000);
        let inner = ctx.push_with_metrics("inner".into(), vec![], 0, 0, 1000, 1000);
        ctx.pop_with_metrics(inner, 0, 100, 1100, 1100);
        ctx.pop_with_metrics(outer, 0, 150, 1200, 1200);
    }
    let tree = ctx.finalize();
    let owned = tree.as_ref().clone();
    let bytes = export_xhprof(&owned, XhprofMode::Raw, None);
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let entry = v.get("outer==>inner").unwrap().as_object().unwrap();
    assert_eq!(entry.get("ct").unwrap().as_u64(), Some(3));
}

#[test]
fn xhgui_mode_wraps_in_envelope_with_meta() {
    let tree = build_simple_tree();
    let meta = XhguiMeta {
        url: "/api/users/42".into(),
        request_method: "GET".into(),
        request_ts: 1_700_000_000,
        request_ts_micro: 1_700_000_000.123_456,
        ..Default::default()
    };
    let bytes = export_xhprof(&tree, XhprofMode::Xhgui, Some(meta));
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(v.get("profile").is_some(), "xhgui has profile field");
    let m = v.get("meta").unwrap().as_object().unwrap();
    assert_eq!(m.get("url").unwrap().as_str(), Some("/api/users/42"));
    assert_eq!(m.get("request_method").unwrap().as_str(), Some("GET"));
    assert_eq!(m.get("request_ts").unwrap().as_u64(), Some(1_700_000_000));
}

#[test]
fn empty_tree_yields_empty_pair_map() {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
    let tree = ctx.finalize().as_ref().clone();
    let bytes = export_xhprof(&tree, XhprofMode::Raw, None);
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    assert!(v.as_object().unwrap().is_empty());
}

#[test]
fn metric_units_microseconds_for_wt_cpu_and_bytes_for_mu_pmu() {
    // All values (including wt) are now deterministic: start_ns/end_ns
    // flow through from explicit args instead of the wall clock.
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
    let id = ctx.push_with_metrics("op".into(), vec![], 0, 1_000_000, 100, 200);
    ctx.pop_with_metrics(id, 0, 1_500_000, 250, 400);
    let tree = ctx.finalize().as_ref().clone();
    let bytes = export_xhprof(&tree, XhprofMode::Raw, None);
    let v: Value = serde_json::from_slice(&bytes).unwrap();
    let entry = v.get("main()==>op").unwrap().as_object().unwrap();
    assert_eq!(
        entry.get("cpu").unwrap().as_u64(),
        Some(500),
        "cpu = 500_000 ns / 1000 = 500 µs"
    );
    assert_eq!(
        entry.get("mu").unwrap().as_i64(),
        Some(150),
        "exit 250 - enter 100"
    );
    // mem_peak = max(mem_peak_enter=200, mem_peak_exit=400) = 400;
    // pmu = peak - enter = 400 - 100 = 300.
    assert_eq!(entry.get("pmu").unwrap().as_i64(), Some(300));
}
