//! Integration tests for DiskWriter.

#![cfg(feature = "plugin-profiler")]

use std::sync::Arc;
use tempfile::TempDir;

use oxphp::plugins::ox_profiler::storage::{DiskWriter, OutputFormat, RunMeta, StorageMetrics};
use oxphp::plugins::ox_profiler::trigger::ActivationSource;
use oxphp::profiling::{FinishedSpan, ProfilingMode, SpanTree};

fn fixture_tree() -> Arc<SpanTree> {
    Arc::new(SpanTree {
        finished: vec![FinishedSpan {
            local_id: 1,
            trace_id: "trace".into(),
            span_id: "s1".into(),
            parent_span_id: "root".into(),
            name: "outer".into(),
            start_ns: 1_000_000_000,
            end_ns: 1_001_000_000,
            attributes: vec![],
            events: vec![],
            status_code: 0,
            status_message: None,
            leaked: false,
            cpu_ns: 500_000,
            mem_enter: 0,
            mem_exit: 0,
            mem_peak: 0,
        }],
        trace_id: "trace".into(),
        root_span_id: "root".into(),
        mode: ProfilingMode::ProfileAll,
    })
}

fn fixture_meta(run_id: &str) -> RunMeta {
    RunMeta {
        run_id: run_id.into(),
        request_id: "req-1".into(),
        trace_id: Some("trace".into()),
        timestamp_ms: 1_700_000_000_000,
        duration_ms: 1,
        method: "GET".into(),
        url: "/test".into(),
        status: 200,
        user_agent: None,
        client_ip: None,
        source: ActivationSource::Header,
        span_count: 1,
        event_count: 0,
        error_count: 0,
        leaked_count: 0,
        truncated: false,
        oxphp_version: "0.2.0".into(),
        formats: vec![],
    }
}

#[tokio::test]
async fn writes_all_four_formats() {
    let dir = TempDir::new().unwrap();
    let writer = DiskWriter::new(
        dir.path().to_path_buf(),
        100.0,
        100.0,
        StorageMetrics::new(),
    );
    let tree = fixture_tree();
    let meta = fixture_meta("run-abc");
    let formats = [
        OutputFormat::Collapsed,
        OutputFormat::Xhprof,
        OutputFormat::Speedscope,
        OutputFormat::Pprof,
    ];
    let ok = writer.write_run(&meta, &tree, &formats, false).await;
    assert!(ok);
    for ext in &["collapsed", "xhprof.json", "speedscope.json", "pprof"] {
        let p = dir.path().join(format!("run-abc.{ext}"));
        assert!(p.exists(), "{} should exist", p.display());
    }
    let index = std::fs::read_to_string(dir.path().join("index.json")).unwrap();
    assert!(
        index.contains("run-abc"),
        "index.json should contain the run_id"
    );
    assert_eq!(index.lines().count(), 1, "exactly one entry per write_run");
}

#[tokio::test]
async fn rate_limit_drops_excess() {
    let dir = TempDir::new().unwrap();
    // capacity=1, rate=0 → only one write succeeds, second is dropped.
    let writer = DiskWriter::new(dir.path().to_path_buf(), 0.0, 1.0, StorageMetrics::new());
    let tree = fixture_tree();
    let m1 = fixture_meta("run-1");
    let m2 = fixture_meta("run-2");
    let ok1 = writer
        .write_run(&m1, &tree, &[OutputFormat::Collapsed], false)
        .await;
    let ok2 = writer
        .write_run(&m2, &tree, &[OutputFormat::Collapsed], false)
        .await;
    assert!(ok1, "first write succeeds");
    assert!(!ok2, "second write rate-limited");
    assert!(dir.path().join("run-1.collapsed").exists());
    assert!(!dir.path().join("run-2.collapsed").exists());
}

#[tokio::test]
async fn rejects_unsafe_run_id() {
    let dir = TempDir::new().unwrap();
    let writer = DiskWriter::new(
        dir.path().to_path_buf(),
        100.0,
        100.0,
        StorageMetrics::new(),
    );
    let tree = fixture_tree();
    let mut meta = fixture_meta("safe");
    meta.run_id = "../etc/passwd".into();
    let ok = writer
        .write_run(&meta, &tree, &[OutputFormat::Collapsed], false)
        .await;
    assert!(!ok, "unsafe run_id rejected");
    // The writer must not have created any traversal-escape file.
    assert!(!dir.path().join("../etc/passwd.collapsed").exists());
}

#[tokio::test]
async fn xhgui_envelope_renders_meta_field() {
    let dir = TempDir::new().unwrap();
    let writer = DiskWriter::new(
        dir.path().to_path_buf(),
        100.0,
        100.0,
        StorageMetrics::new(),
    );
    let tree = fixture_tree();
    let meta = fixture_meta("run-xhgui");
    writer
        .write_run(&meta, &tree, &[OutputFormat::Xhprof], true)
        .await;
    let body = std::fs::read_to_string(dir.path().join("run-xhgui.xhprof.json")).unwrap();
    assert!(body.contains("\"meta\""), "xhgui envelope present");
    assert!(body.contains("\"profile\""));
}
