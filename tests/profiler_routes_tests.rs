//! Integration tests for the profiler internal routes.
#![cfg(feature = "plugin-profiler")]

use std::sync::Arc;

use bytes::Bytes;
use http::StatusCode;
use tempfile::TempDir;

use oxphp::plugin::handler::{PluginInternalHandler, PluginInternalRequest};
use oxphp::plugins::ox_profiler::routes;
use oxphp::plugins::ox_profiler::storage::{
    DiskWriter, ProfileCache, RunMeta, Storage, StorageMetrics,
};
use oxphp::plugins::ox_profiler::trigger::ActivationSource;
use oxphp::profiling::{ProfilingMode, SpanTree};

fn storage_with_disk(dir: &std::path::Path) -> Arc<Storage> {
    let metrics = StorageMetrics::new();
    let cache = Arc::new(ProfileCache::new(16));
    let disk = DiskWriter::new(dir.to_path_buf(), 100.0, 100.0, Arc::clone(&metrics));
    Arc::new(Storage {
        cache,
        disk: Some(Arc::new(disk)),
        http: None,
        metrics,
    })
}

fn build_router(storage: Arc<Storage>, auth: Option<&str>) -> Arc<dyn PluginInternalHandler> {
    routes::test_new_router(
        storage,
        auth.map(Arc::<str>::from),
        serde_json::json!({ "enabled": true }),
    )
}

fn write_index(dir: &std::path::Path, metas: &[RunMeta]) {
    let mut s = String::new();
    for m in metas {
        s.push_str(&serde_json::to_string(m).unwrap());
        s.push('\n');
    }
    std::fs::write(dir.join("index.json"), s).unwrap();
}

fn meta(run_id: &str, ts: u64, formats: &[&str]) -> RunMeta {
    RunMeta {
        run_id: run_id.into(),
        request_id: run_id.into(),
        trace_id: None,
        timestamp_ms: ts,
        duration_ms: 10,
        method: "GET".into(),
        url: "/".into(),
        status: 200,
        user_agent: None,
        client_ip: None,
        source: ActivationSource::Header,
        span_count: 3,
        event_count: 0,
        error_count: 0,
        leaked_count: 0,
        truncated: false,
        oxphp_version: "0.2.0".into(),
        formats: formats.iter().map(|s| (*s).into()).collect(),
    }
}

fn issue(
    router: &dyn PluginInternalHandler,
    method: &http::Method,
    path: &str,
    token: Option<&str>,
    query: Option<&str>,
) -> http::Response<oxphp::types::ResponseBody> {
    let mut headers = http::HeaderMap::new();
    if let Some(t) = token {
        headers.insert("authorization", format!("Bearer {}", t).parse().unwrap());
    }
    headers.insert("host", "127.0.0.1:9090".parse().unwrap());
    let r = PluginInternalRequest {
        method,
        path,
        headers: &headers,
        query,
    };
    router.handle(&r)
}

fn body_to_bytes(resp: http::Response<oxphp::types::ResponseBody>) -> Bytes {
    use http_body_util::BodyExt;
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    rt.block_on(async move { resp.into_body().collect().await.unwrap().to_bytes() })
}

#[test]
fn list_runs_returns_paginated_json() {
    let tmp = TempDir::new().unwrap();
    write_index(
        tmp.path(),
        &[
            meta("a", 100, &["collapsed"]),
            meta("b", 200, &["collapsed"]),
            meta("c", 300, &["collapsed"]),
        ],
    );
    let storage = storage_with_disk(tmp.path());
    let router = build_router(storage, None);
    let resp = issue(
        &*router,
        &http::Method::GET,
        "/__profiler/runs",
        None,
        Some("limit=2"),
    );
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_to_bytes(resp);
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(v["total"], 3);
    assert_eq!(v["limit"], 2);
    assert_eq!(v["runs"].as_array().unwrap().len(), 2);
    assert_eq!(v["runs"][0]["run_id"], "c");
    assert_eq!(v["runs"][1]["run_id"], "b");
}

#[test]
fn run_metadata_404_when_missing() {
    let tmp = TempDir::new().unwrap();
    write_index(tmp.path(), &[meta("a", 1, &["collapsed"])]);
    let storage = storage_with_disk(tmp.path());
    let router = build_router(storage, None);
    let resp = issue(
        &*router,
        &http::Method::GET,
        "/__profiler/runs/does-not-exist",
        None,
        None,
    );
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[test]
fn run_format_bytes_disk_fallback() {
    let tmp = TempDir::new().unwrap();
    write_index(tmp.path(), &[meta("abc", 1, &["collapsed"])]);
    std::fs::write(tmp.path().join("abc.collapsed"), b"main;inner 42\n").unwrap();
    let storage = storage_with_disk(tmp.path());
    let router = build_router(storage, None);
    let resp = issue(
        &*router,
        &http::Method::GET,
        "/__profiler/runs/abc.collapsed",
        None,
        None,
    );
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_to_bytes(resp);
    assert_eq!(&body[..], b"main;inner 42\n");
}

#[test]
fn run_format_cache_wins_over_disk() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("abc.collapsed"), b"stale").unwrap();
    let storage = storage_with_disk(tmp.path());
    let tree = Arc::new(SpanTree {
        finished: vec![],
        trace_id: "t".into(),
        root_span_id: "r".into(),
        mode: ProfilingMode::ProfileAll,
    });
    storage.cache.put("abc".into(), Arc::clone(&tree));
    let router = build_router(storage, None);
    let resp = issue(
        &*router,
        &http::Method::GET,
        "/__profiler/runs/abc.collapsed",
        None,
        None,
    );
    assert_eq!(resp.status(), StatusCode::OK);
    let body = body_to_bytes(resp);
    assert_ne!(
        &body[..],
        b"stale",
        "cache re-export took precedence over disk"
    );
}

#[test]
fn speedscope_redirect_sets_location() {
    let tmp = TempDir::new().unwrap();
    let storage = storage_with_disk(tmp.path());
    let router = build_router(storage, None);
    let resp = issue(
        &*router,
        &http::Method::GET,
        "/__profiler/runs/abc/speedscope",
        None,
        None,
    );
    assert_eq!(resp.status(), StatusCode::FOUND);
    let loc = resp
        .headers()
        .get(http::header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(loc.starts_with("https://www.speedscope.app/#profileURL="));
    assert!(loc.contains("%2F__profiler%2Fruns%2Fabc.speedscope.json"));
    assert!(loc.contains("127.0.0.1"));
}

#[test]
fn delete_removes_run_and_index_entry() {
    let tmp = TempDir::new().unwrap();
    write_index(
        tmp.path(),
        &[meta("a", 1, &["collapsed"]), meta("b", 2, &["collapsed"])],
    );
    std::fs::write(tmp.path().join("a.collapsed"), b"x").unwrap();
    std::fs::write(tmp.path().join("b.collapsed"), b"y").unwrap();
    let storage = storage_with_disk(tmp.path());
    let router = build_router(storage, None);
    let resp = issue(
        &*router,
        &http::Method::DELETE,
        "/__profiler/runs/a",
        None,
        None,
    );
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(!tmp.path().join("a.collapsed").exists());
    assert!(tmp.path().join("b.collapsed").exists());
    let idx = std::fs::read_to_string(tmp.path().join("index.json")).unwrap();
    assert!(!idx.contains("\"run_id\":\"a\""));
    assert!(idx.contains("\"run_id\":\"b\""));
}

#[test]
fn auth_enforced_on_stats_when_token_configured() {
    let tmp = TempDir::new().unwrap();
    let storage = storage_with_disk(tmp.path());
    let router = build_router(storage, Some("s3cret"));

    let resp = issue(
        &*router,
        &http::Method::GET,
        "/__profiler/stats",
        None,
        None,
    );
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = issue(
        &*router,
        &http::Method::GET,
        "/__profiler/stats",
        Some("nope"),
        None,
    );
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);

    let resp = issue(
        &*router,
        &http::Method::GET,
        "/__profiler/stats",
        Some("s3cret"),
        None,
    );
    assert_eq!(resp.status(), StatusCode::OK);
}

#[test]
fn path_traversal_run_id_rejected() {
    let tmp = TempDir::new().unwrap();
    let storage = storage_with_disk(tmp.path());
    let router = build_router(storage, None);
    let resp = issue(
        &*router,
        &http::Method::GET,
        "/__profiler/runs/../etc/passwd",
        None,
        None,
    );
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}
