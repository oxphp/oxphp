//! Golden-fixture tests across the exporters.
//!
//! The "tiny" tree is hand-constructed via direct FinishedSpan
//! literals so timestamps, cpu_ns, and memory values are stable
//! across test runs (bypasses now_ns()).
//!
//! When you intentionally change a wire format, regenerate the
//! fixtures via:
//!
//!   cargo test --features plugin-profiler --no-default-features \
//!     --test profiler_export_fixtures_tests \
//!     regenerate_fixtures -- --ignored --nocapture
//!
//! Then commit the diff with a clear "intentional format change"
//! justification.

#![cfg(feature = "plugin-profiler")]

use std::path::PathBuf;

use oxphp::profiling::export::{export_collapsed, export_xhprof};
use oxphp::profiling::{FinishedSpan, ProfilingMode, SpanTree};

/// Build the deterministic fixture tree.
///
/// Tree shape (timestamps in ns, monotonically increasing):
///
///   outer  start=1_000_000_000 end=1_001_000_000  cpu=500µs  mem +1000B
///   └── middle  start=1_000_200_000 end=1_000_700_000  cpu=300µs  mem +500B
///       └── inner  start=1_000_300_000 end=1_000_500_000  cpu=100µs  mem ±0
///
/// Finalize order: leaf-first, so finished[0] = inner.
fn make_fixture_tree() -> SpanTree {
    SpanTree {
        finished: vec![
            FinishedSpan {
                local_id: 3,
                trace_id: "trace-fixture".into(),
                span_id: "spaninner".into(),
                parent_span_id: "spanmiddle".into(),
                name: "inner".into(),
                start_ns: 1_000_300_000,
                end_ns: 1_000_500_000,
                attributes: vec![],
                events: vec![],
                status_code: 0,
                status_message: None,
                leaked: false,
                cpu_ns: 100_000,
                mem_enter: 5_000,
                mem_exit: 5_000,
                mem_peak: 5_500,
            },
            FinishedSpan {
                local_id: 2,
                trace_id: "trace-fixture".into(),
                span_id: "spanmiddle".into(),
                parent_span_id: "spanouter".into(),
                name: "middle".into(),
                start_ns: 1_000_200_000,
                end_ns: 1_000_700_000,
                attributes: vec![],
                events: vec![],
                status_code: 0,
                status_message: None,
                leaked: false,
                cpu_ns: 300_000,
                mem_enter: 4_500,
                mem_exit: 5_000,
                mem_peak: 5_500,
            },
            FinishedSpan {
                local_id: 1,
                trace_id: "trace-fixture".into(),
                span_id: "spanouter".into(),
                parent_span_id: "root-fixture".into(),
                name: "outer".into(),
                start_ns: 1_000_000_000,
                end_ns: 1_001_000_000,
                attributes: vec![],
                events: vec![],
                status_code: 0,
                status_message: None,
                leaked: false,
                cpu_ns: 500_000,
                mem_enter: 4_000,
                mem_exit: 5_000,
                mem_peak: 5_500,
            },
        ],
        trace_id: "trace-fixture".into(),
        root_span_id: "root-fixture".into(),
        mode: ProfilingMode::ProfileAll,
        truncated: false,
    }
}

fn fixture_path(name: &str) -> PathBuf {
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.push("tests/fixtures/profiler_exports");
    p.push(name);
    p
}

fn read_fixture(name: &str) -> Vec<u8> {
    std::fs::read(fixture_path(name))
        .unwrap_or_else(|e| panic!("missing fixture {name}: {e}\n\nRun the regenerate_fixtures test once to create the fixture files."))
}

// ── Byte / semantic compare tests ─────────────────────────────

#[test]
fn collapsed_wall_matches_fixture() {
    let tree = make_fixture_tree();
    let out = export_collapsed(&tree);
    let expected = read_fixture("tiny.collapsed");
    assert_eq!(
        std::str::from_utf8(&out).unwrap(),
        std::str::from_utf8(&expected).unwrap(),
        "collapsed Wall output drifted from fixture"
    );
}

#[test]
fn xhprof_matches_fixture() {
    let tree = make_fixture_tree();
    let out = export_xhprof(&tree);
    let expected = read_fixture("tiny.xhprof.json");
    let actual_v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let expected_v: serde_json::Value = serde_json::from_slice(&expected).unwrap();
    assert_eq!(actual_v, expected_v, "xhprof output drifted from fixture");
}

// ── Regenerate helper ──────────────────────────────────────────

/// Writes / overwrites the fixture files. Run intentionally when a
/// wire format changes:
///
///     cargo test --features plugin-profiler --no-default-features \
///       --test profiler_export_fixtures_tests \
///       regenerate_fixtures -- --ignored --nocapture
///
/// Eyeball the diff before committing the new fixtures.
#[test]
#[ignore]
fn regenerate_fixtures() {
    let tree = make_fixture_tree();
    let dir = fixture_path("");
    std::fs::create_dir_all(&dir).unwrap();
    let write = |name: &str, bytes: Vec<u8>| {
        let path = fixture_path(name);
        std::fs::write(&path, &bytes).unwrap();
        println!("wrote {} ({} bytes)", path.display(), bytes.len());
    };
    write("tiny.collapsed", export_collapsed(&tree));
    write("tiny.xhprof.json", export_xhprof(&tree));
}
