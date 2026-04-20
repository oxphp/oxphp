//! Integration tests for the collapsed-stack exporter.

#![cfg(feature = "plugin-profiler")]

use oxphp::profiling::export::{export_collapsed, CollapsedMetric};
use oxphp::profiling::{ProfilingContext, ProfilingMode};

/// Builds a 3-span tree shaped outer → middle → inner with
/// monotonically growing CPU and memory readings.
fn build_simple_tree() -> oxphp::profiling::SpanTree {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
    // push_with_metrics signature: (name, attrs, start_ns, cpu_start_ns, mem_enter, mem_peak_enter)
    // pop_with_metrics  signature: (id,          end_ns,  cpu_end_ns,   mem_exit,  mem_peak_exit)
    let outer = ctx.push_with_metrics("outer".into(), vec![], 1_000, 0, 1000, 1500);
    let middle = ctx.push_with_metrics("middle".into(), vec![], 2_000, 100, 1100, 1600);
    let inner = ctx.push_with_metrics("inner".into(), vec![], 3_000, 200, 1200, 1700);
    ctx.pop_with_metrics(inner, 4_000, 250, 1300, 1800);
    ctx.pop_with_metrics(middle, 5_000, 280, 1400, 1900);
    ctx.pop_with_metrics(outer, 6_000, 320, 1500, 2000);
    ctx.finalize().as_ref().clone()
}

#[test]
fn wall_metric_emits_one_line_per_span() {
    let tree = build_simple_tree();
    let bytes = export_collapsed(&tree, CollapsedMetric::Wall);
    let text = std::str::from_utf8(&bytes).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "one line per finished span");
    assert!(lines.iter().all(|l| l.starts_with("main();")));
}

#[test]
fn paths_are_root_first_with_synthetic_main() {
    let tree = build_simple_tree();
    let bytes = export_collapsed(&tree, CollapsedMetric::Wall);
    let text = std::str::from_utf8(&bytes).unwrap();

    // Inner is the deepest — its line must contain all three names.
    assert!(
        text.contains("main();outer;middle;inner "),
        "actual:\n{text}"
    );
    assert!(text.contains("main();outer;middle "), "actual:\n{text}");
    assert!(text.contains("main();outer "), "actual:\n{text}");
}

#[test]
fn cpu_metric_emits_only_spans_with_nonzero_cpu() {
    let tree = build_simple_tree();
    let bytes = export_collapsed(&tree, CollapsedMetric::Cpu);
    let text = std::str::from_utf8(&bytes).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    // All three spans have non-zero cpu (delta = 50 / 180 / 320). All emit.
    assert_eq!(lines.len(), 3);
}

#[test]
fn cpu_metric_with_zero_cpu_emits_nothing() {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ApmOnly, "trace".into(), "root".into());
    let id = ctx.push("apm-style".into(), vec![]); // no metrics → cpu_ns=0
    ctx.pop(id);
    let tree = ctx.finalize();
    let bytes = export_collapsed(&tree, CollapsedMetric::Cpu);
    assert!(bytes.is_empty(), "no CPU data → no lines");
}

#[test]
fn mem_metric_skips_negative_or_zero_delta() {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
    let a = ctx.push_with_metrics("alloc".into(), vec![], 0, 0, 1000, 0);
    ctx.pop_with_metrics(a, 0, 0, 5000, 0); // +4000 bytes — emits
    let b = ctx.push_with_metrics("free".into(), vec![], 0, 0, 5000, 0);
    ctx.pop_with_metrics(b, 0, 0, 1000, 0); // -4000 bytes — skipped
    let tree = ctx.finalize();
    let bytes = export_collapsed(&tree, CollapsedMetric::Mem);
    let text = std::str::from_utf8(&bytes).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1);
    assert!(lines[0].contains("alloc"));
    assert!(lines[0].ends_with(" 4000"));
}

#[test]
fn empty_tree_yields_empty_output() {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
    let tree = ctx.finalize();
    assert!(export_collapsed(&tree, CollapsedMetric::Wall).is_empty());
    assert!(export_collapsed(&tree, CollapsedMetric::Cpu).is_empty());
    assert!(export_collapsed(&tree, CollapsedMetric::Mem).is_empty());
}

#[test]
fn semicolon_in_name_is_escaped() {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
    let id = ctx.push_with_metrics("weird;name".into(), vec![], 0, 0, 0, 0);
    ctx.pop_with_metrics(id, 0, 0, 0, 0);
    let tree = ctx.finalize();
    let bytes = export_collapsed(&tree, CollapsedMetric::Wall);
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("weird\\;name"), "actual:\n{text}");
}
