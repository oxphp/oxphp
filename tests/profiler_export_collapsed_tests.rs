//! Integration tests for the collapsed-stack exporter.

#![cfg(feature = "plugin-profiler")]

use oxphp::profiling::export::export_collapsed;
use oxphp::profiling::{ProfilingContext, ProfilingMode};

/// Builds a 3-span tree shaped outer → middle → inner.
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
fn each_distinct_stack_gets_one_line() {
    let tree = build_simple_tree();
    let bytes = export_collapsed(&tree);
    let text = std::str::from_utf8(&bytes).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 3, "one line per distinct stack");
    assert!(lines.iter().all(|l| l.starts_with("main();")));
}

#[test]
fn paths_are_root_first_with_synthetic_main() {
    let tree = build_simple_tree();
    let bytes = export_collapsed(&tree);
    let text = std::str::from_utf8(&bytes).unwrap();

    // Inner is the deepest — its line must contain all three names.
    assert!(
        text.contains("main();outer;middle;inner "),
        "actual:\n{text}"
    );
    assert!(text.contains("main();outer;middle "), "actual:\n{text}");
    assert!(text.contains("main();outer "), "actual:\n{text}");
}

/// A flamegraph draws a frame as wide as its own line plus every line under
/// it, so a line carries the call without its callees.
#[test]
fn each_line_counts_the_call_without_its_callees() {
    let tree = build_simple_tree();
    let bytes = export_collapsed(&tree);
    let text = std::str::from_utf8(&bytes).unwrap();

    assert!(text.contains("main();outer;middle;inner 1\n"), "{text}");
    assert!(text.contains("main();outer;middle 2\n"), "{text}");
    assert!(text.contains("main();outer 2\n"), "{text}");
}

/// Ten calls of 900 ns are 9 µs. Cut to microseconds per call they would be
/// ten zeros, and the caller has already given those nanoseconds up.
#[test]
fn calls_shorter_than_a_microsecond_add_up_on_their_stack() {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
    let outer = ctx.push_with_metrics("outer".into(), vec![], 0, 0, 0, 0);
    for i in 0..10u64 {
        let start = 100 + i * 1_000;
        let leaf = ctx.push_with_metrics("leaf".into(), vec![], start, 0, 0, 0);
        ctx.pop_with_metrics(leaf, start + 900, 0, 0, 0);
    }
    ctx.pop_with_metrics(outer, 10_000, 0, 0, 0);
    let tree = ctx.finalize();

    let bytes = export_collapsed(&tree);
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("main();outer;leaf 9\n"), "{text}");
    assert!(text.contains("main();outer 1\n"), "{text}");
    assert_eq!(text.lines().count(), 2, "{text}");
}

#[test]
fn sibling_callees_are_all_taken_out_of_the_caller() {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
    let outer = ctx.push_with_metrics("outer".into(), vec![], 0, 0, 0, 0);
    let first = ctx.push_with_metrics("first".into(), vec![], 1_000, 0, 0, 0);
    ctx.pop_with_metrics(first, 3_000, 0, 0, 0);
    let second = ctx.push_with_metrics("second".into(), vec![], 4_000, 0, 0, 0);
    ctx.pop_with_metrics(second, 7_000, 0, 0, 0);
    ctx.pop_with_metrics(outer, 10_000, 0, 0, 0);
    let tree = ctx.finalize();

    let bytes = export_collapsed(&tree);
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("main();outer 5\n"), "{text}");
}

/// A force-closed span can read as far longer than its caller, because its
/// end comes from another clock. The caller is then 0, not a wrapped integer.
#[test]
fn callee_longer_than_its_caller_leaves_the_caller_at_zero() {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
    let outer = ctx.push_with_metrics("outer".into(), vec![], 1_000, 0, 0, 0);
    for _ in 0..12 {
        let child = ctx.push_with_metrics("leaked".into(), vec![], 0, 0, 0, 0);
        ctx.pop_with_metrics(child, u64::MAX / 8, 0, 0, 0);
    }
    ctx.pop_with_metrics(outer, 2_000, 0, 0, 0);
    let tree = ctx.finalize();

    let bytes = export_collapsed(&tree);
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("main();outer 0\n"), "{text}");
}

#[test]
fn empty_tree_yields_empty_output() {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
    let tree = ctx.finalize();
    assert!(export_collapsed(&tree).is_empty());
}

#[test]
fn semicolon_in_name_is_escaped() {
    let mut ctx = ProfilingContext::new();
    ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
    let id = ctx.push_with_metrics("weird;name".into(), vec![], 0, 0, 0, 0);
    ctx.pop_with_metrics(id, 0, 0, 0, 0);
    let tree = ctx.finalize();
    let bytes = export_collapsed(&tree);
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("weird\\;name"), "actual:\n{text}");
}
