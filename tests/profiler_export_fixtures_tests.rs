//! Golden-fixture tests across all four exporters.
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

use oxphp::profiling::export::{
    export_collapsed, export_pprof, export_speedscope, export_xhprof, CollapsedMetric, XhguiMeta,
    XhprofMode,
};
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
    }
}

fn fixture_meta() -> XhguiMeta {
    XhguiMeta {
        url: "/fixture".into(),
        request_method: "GET".into(),
        request_ts: 1_700_000_000,
        request_ts_micro: 1_700_000_000.0,
        ..Default::default()
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
    let out = export_collapsed(&tree, CollapsedMetric::Wall);
    let expected = read_fixture("tiny.collapsed");
    assert_eq!(
        std::str::from_utf8(&out).unwrap(),
        std::str::from_utf8(&expected).unwrap(),
        "collapsed Wall output drifted from fixture"
    );
}

#[test]
fn collapsed_cpu_matches_fixture() {
    let tree = make_fixture_tree();
    let out = export_collapsed(&tree, CollapsedMetric::Cpu);
    let expected = read_fixture("tiny.collapsed.cpu");
    assert_eq!(
        std::str::from_utf8(&out).unwrap(),
        std::str::from_utf8(&expected).unwrap()
    );
}

#[test]
fn collapsed_mem_matches_fixture() {
    let tree = make_fixture_tree();
    let out = export_collapsed(&tree, CollapsedMetric::Mem);
    let expected = read_fixture("tiny.collapsed.mem");
    assert_eq!(
        std::str::from_utf8(&out).unwrap(),
        std::str::from_utf8(&expected).unwrap()
    );
}

#[test]
fn xhprof_raw_matches_fixture() {
    let tree = make_fixture_tree();
    let out = export_xhprof(&tree, XhprofMode::Raw, None);
    let expected = read_fixture("tiny.xhprof.json");
    let actual_v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let expected_v: serde_json::Value = serde_json::from_slice(&expected).unwrap();
    assert_eq!(actual_v, expected_v, "xhprof Raw drifted from fixture");
}

#[test]
fn xhprof_xhgui_matches_fixture() {
    let tree = make_fixture_tree();
    let out = export_xhprof(&tree, XhprofMode::Xhgui, Some(fixture_meta()));
    let expected = read_fixture("tiny.xhprof.xhgui.json");
    let actual_v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let expected_v: serde_json::Value = serde_json::from_slice(&expected).unwrap();
    assert_eq!(actual_v, expected_v, "xhprof Xhgui drifted from fixture");
}

#[test]
fn speedscope_matches_fixture() {
    let tree = make_fixture_tree();
    let out = export_speedscope(&tree);
    let expected = read_fixture("tiny.speedscope.json");
    let actual_v: serde_json::Value = serde_json::from_slice(&out).unwrap();
    let expected_v: serde_json::Value = serde_json::from_slice(&expected).unwrap();
    assert_eq!(actual_v, expected_v, "speedscope drifted from fixture");
}

// ── pprof structural decode-compare ───────────────────────────

mod pprof_proto {
    #![allow(clippy::all, dead_code)]
    include!(concat!(env!("OUT_DIR"), "/perftools.profiles.rs"));
}

#[test]
fn pprof_decodes_to_expected_shape() {
    use prost::Message;
    use std::io::Read;

    let tree = make_fixture_tree();
    let gz = export_pprof(&tree);

    // Decompress + decode round-trip.
    let mut decoder = flate2::read::GzDecoder::new(&gz[..]);
    let mut raw = Vec::new();
    decoder.read_to_end(&mut raw).expect("gunzip");

    let profile = pprof_proto::Profile::decode(&raw[..]).expect("valid pprof.proto");

    // 4 sample types, in declared order: wall, cpu, alloc_space, inuse_space.
    assert_eq!(profile.sample_type.len(), 4);
    let st_names: Vec<&str> = profile
        .sample_type
        .iter()
        .map(|st| profile.string_table[st.r#type as usize].as_str())
        .collect();
    assert_eq!(st_names, vec!["wall", "cpu", "alloc_space", "inuse_space"]);

    // Only the leaf (`inner`) contributes a sample.
    assert_eq!(profile.sample.len(), 1);
    let sample = &profile.sample[0];

    // Sample values: wt = (1_000_500_000 - 1_000_300_000) / 1000 = 200µs;
    // cpu = 100_000 / 1000 = 100µs; alloc = 0; inuse = 5_500 - 5_000 = 500.
    assert_eq!(sample.value[0], 200, "wall µs");
    assert_eq!(sample.value[1], 100, "cpu µs");
    assert_eq!(sample.value[2], 0, "alloc_space (no delta on inner)");
    assert_eq!(sample.value[3], 500, "inuse_space (peak - enter)");

    // Stack: inner → middle → outer = 3 locations, leaf-first.
    assert_eq!(sample.location_id.len(), 3);

    // Function table contains 3 unique names.
    assert_eq!(profile.function.len(), 3);
    let names: Vec<&str> = profile
        .function
        .iter()
        .map(|f| profile.string_table[f.name as usize].as_str())
        .collect();
    assert!(names.contains(&"inner"));
    assert!(names.contains(&"middle"));
    assert!(names.contains(&"outer"));

    // Default sample type points at "wall".
    assert_eq!(
        profile.string_table[profile.default_sample_type as usize],
        "wall"
    );

    // time_nanos = earliest start_ns = 1_000_000_000.
    assert_eq!(profile.time_nanos, 1_000_000_000);
    // duration_nanos = max_end - min_start = 1_001_000_000 - 1_000_000_000.
    assert_eq!(profile.duration_nanos, 1_000_000);
}

#[test]
fn pprof_empty_tree_yields_valid_profile() {
    use prost::Message;
    use std::io::Read;

    let tree = SpanTree {
        finished: vec![],
        trace_id: "empty".into(),
        root_span_id: "root".into(),
        mode: ProfilingMode::ProfileAll,
    };
    let gz = export_pprof(&tree);
    let mut decoder = flate2::read::GzDecoder::new(&gz[..]);
    let mut raw = Vec::new();
    decoder.read_to_end(&mut raw).unwrap();
    let profile = pprof_proto::Profile::decode(&raw[..]).expect("valid empty pprof");
    assert_eq!(profile.sample_type.len(), 4);
    assert_eq!(profile.sample.len(), 0);
    assert_eq!(profile.time_nanos, 0);
    assert_eq!(profile.duration_nanos, 0);
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
    write(
        "tiny.collapsed",
        export_collapsed(&tree, CollapsedMetric::Wall),
    );
    write(
        "tiny.collapsed.cpu",
        export_collapsed(&tree, CollapsedMetric::Cpu),
    );
    write(
        "tiny.collapsed.mem",
        export_collapsed(&tree, CollapsedMetric::Mem),
    );
    write(
        "tiny.xhprof.json",
        export_xhprof(&tree, XhprofMode::Raw, None),
    );
    write(
        "tiny.xhprof.xhgui.json",
        export_xhprof(&tree, XhprofMode::Xhgui, Some(fixture_meta())),
    );
    write("tiny.speedscope.json", export_speedscope(&tree));
}
