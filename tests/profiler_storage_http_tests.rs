//! Integration tests for HttpPusher.

#![cfg(feature = "plugin-profiler")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use oxphp::plugins::ox_profiler::storage::{HttpPusher, OutputFormat, RunMeta, StorageMetrics};
use oxphp::plugins::ox_profiler::trigger::ActivationSource;
use oxphp::profiling::export::BuggregatorMeta;
use oxphp::profiling::{ProfilingMode, SpanTree};

/// Spin a minimal mock HTTP server returning 200 OK. Returns
/// (port, hits_counter, captured_first_request).
async fn spawn_mock_server() -> (u16, Arc<AtomicUsize>, Arc<tokio::sync::Mutex<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Arc::new(AtomicUsize::new(0));
    let captured = Arc::new(tokio::sync::Mutex::new(String::new()));
    let h_clone = Arc::clone(&hits);
    let c_clone = Arc::clone(&captured);
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let h = Arc::clone(&h_clone);
            let c = Arc::clone(&c_clone);
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]).to_string();
                if h.fetch_add(1, Ordering::SeqCst) == 0 {
                    *c.lock().await = req;
                }
                let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n";
                let _ = sock.write_all(resp).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    (port, hits, captured)
}

fn empty_tree() -> Arc<SpanTree> {
    Arc::new(SpanTree {
        finished: vec![],
        trace_id: "t".into(),
        root_span_id: "r".into(),
        mode: ProfilingMode::ProfileAll,
    })
}

fn empty_meta() -> RunMeta {
    RunMeta {
        run_id: "run-x".into(),
        request_id: "req-x".into(),
        trace_id: None,
        timestamp_ms: 0,
        duration_ms: 0,
        method: "GET".into(),
        url: "/x".into(),
        status: 200,
        user_agent: None,
        client_ip: None,
        source: ActivationSource::Header,
        span_count: 0,
        event_count: 0,
        error_count: 0,
        leaked_count: 0,
        truncated: false,
        oxphp_version: "0.2.0".into(),
        formats: vec![],
    }
}

#[tokio::test]
async fn http_push_happy_path() {
    let (port, hits, captured) = spawn_mock_server().await;
    let pusher = Arc::new(
        HttpPusher::new(
            format!("http://127.0.0.1:{port}/"),
            OutputFormat::Collapsed,
            None,
            false,
            None,
            StorageMetrics::new(),
        )
        .unwrap(),
    );
    pusher.push(&empty_meta(), &empty_tree()).await;
    assert_eq!(hits.load(Ordering::SeqCst), 1);
    let req = captured.lock().await;
    assert!(
        req.to_lowercase().contains("content-type: text/plain"),
        "wrong content type in: {req}"
    );
}

#[tokio::test]
async fn http_push_bearer_auth() {
    let (port, _hits, captured) = spawn_mock_server().await;
    let pusher = Arc::new(
        HttpPusher::new(
            format!("http://127.0.0.1:{port}/"),
            OutputFormat::Collapsed,
            Some("supersecret".into()),
            false,
            None,
            StorageMetrics::new(),
        )
        .unwrap(),
    );
    pusher.push(&empty_meta(), &empty_tree()).await;
    let req = captured.lock().await;
    assert!(
        req.to_lowercase()
            .contains("authorization: bearer supersecret"),
        "missing bearer auth header in: {req}"
    );
}

#[tokio::test]
async fn http_push_xhgui_envelope() {
    // Envelope detection lives in the config layer (unit-tested there); the
    // pusher renders whatever flag it is handed. `true` → the `{profile, meta}`
    // xhgui wrapper.
    let (port, _hits, captured) = spawn_mock_server().await;
    let pusher = Arc::new(
        HttpPusher::new(
            format!("http://127.0.0.1:{port}/run/import"),
            OutputFormat::Xhprof,
            None,
            true,
            None,
            StorageMetrics::new(),
        )
        .unwrap(),
    );
    pusher.push(&empty_meta(), &empty_tree()).await;
    let req = captured.lock().await;
    assert!(req.contains("\"profile\""), "xhgui envelope applied");
    assert!(req.contains("\"meta\""));
}

#[tokio::test]
async fn http_push_buggregator_envelope() {
    let (port, _hits, captured) = spawn_mock_server().await;
    let pusher = Arc::new(
        HttpPusher::new(
            format!("http://127.0.0.1:{port}/api/profiler/store"),
            OutputFormat::Xhprof,
            None,
            false,
            Some(BuggregatorMeta {
                app_name: "shop".to_string(),
                tags: vec![("env".to_string(), "prod".to_string())],
                hostname: "web-1".to_string(),
            }),
            StorageMetrics::new(),
        )
        .unwrap(),
    );
    // Non-zero timestamp so the `date` field is derived (ms → s).
    let mut meta = empty_meta();
    meta.timestamp_ms = 1_700_000_000_000;
    pusher.push(&meta, &empty_tree()).await;

    let req = captured.lock().await;
    // Buggregator's `/api/profiler/store` envelope, not xhgui.
    assert!(req.contains("\"profile\""), "missing profile in: {req}");
    assert!(
        !req.contains("\"meta\""),
        "must not be xhgui envelope: {req}"
    );
    assert!(req.contains("\"app_name\":\"shop\""), "app_name in: {req}");
    assert!(req.contains("\"hostname\":\"web-1\""), "hostname in: {req}");
    assert!(req.contains("\"env\":\"prod\""), "tags in: {req}");
    assert!(req.contains("\"date\":1700000000"), "date (s) in: {req}");
    // Content-Type stays application/json for the xhprof format.
    assert!(
        req.to_lowercase()
            .contains("content-type: application/json"),
        "wrong content type in: {req}"
    );
}

/// An active envelope wins over a non-xhprof `PROFILER_EXPORT_FORMAT`: the
/// pusher emits the Buggregator body, never the raw speedscope the format knob
/// names. Guards against the "auto-detected envelope + speedscope → lost/empty"
/// regression (which previously either crashed startup or shipped un-enveloped).
#[tokio::test]
async fn http_push_buggregator_envelope_wins_over_non_xhprof_format() {
    let (port, _hits, captured) = spawn_mock_server().await;
    let pusher = Arc::new(
        HttpPusher::new(
            format!("http://127.0.0.1:{port}/api/profiler/store"),
            OutputFormat::Speedscope, // ignored while the envelope is active
            None,
            false,
            Some(BuggregatorMeta {
                app_name: "shop".to_string(),
                tags: vec![],
                hostname: "h".to_string(),
            }),
            StorageMetrics::new(),
        )
        .unwrap(),
    );
    pusher.push(&empty_meta(), &empty_tree()).await;
    let req = captured.lock().await;
    assert!(req.contains("\"app_name\""), "not buggregator body: {req}");
    assert!(
        !req.contains("\"$schema\"") && !req.contains("\"shared\""),
        "speedscope leaked despite active envelope: {req}"
    );
}

/// Content-Type follows the rendered body, not the format knob: with an active
/// Buggregator envelope the body is JSON even when the format is pprof, so the
/// header must be `application/json`, never the pprof content type — else a
/// strict receiver rejects the mislabeled push and the profile is lost.
#[tokio::test]
async fn http_push_content_type_follows_envelope_body_not_format() {
    let (port, _hits, captured) = spawn_mock_server().await;
    let pusher = Arc::new(
        HttpPusher::new(
            format!("http://127.0.0.1:{port}/api/profiler/store"),
            OutputFormat::Pprof, // body is still JSON (envelope wins)
            None,
            false,
            Some(BuggregatorMeta {
                app_name: "a".to_string(),
                tags: vec![],
                hostname: "h".to_string(),
            }),
            StorageMetrics::new(),
        )
        .unwrap(),
    );
    pusher.push(&empty_meta(), &empty_tree()).await;
    let req = captured.lock().await.to_lowercase();
    assert!(
        req.contains("content-type: application/json"),
        "header must be json: {req}"
    );
    assert!(!req.contains("pprof"), "must not tag body as pprof: {req}");
    assert!(
        req.contains("\"app_name\""),
        "body is buggregator json: {req}"
    );
}

/// Same invariant for xhgui: envelope wins over a non-xhprof format, so the
/// `{profile, meta}` wrapper is emitted rather than raw speedscope.
#[tokio::test]
async fn http_push_xhgui_envelope_wins_over_non_xhprof_format() {
    let (port, _hits, captured) = spawn_mock_server().await;
    let pusher = Arc::new(
        HttpPusher::new(
            format!("http://127.0.0.1:{port}/run/import"),
            OutputFormat::Speedscope, // ignored while the envelope is active
            None,
            true,
            None,
            StorageMetrics::new(),
        )
        .unwrap(),
    );
    pusher.push(&empty_meta(), &empty_tree()).await;
    let req = captured.lock().await;
    assert!(req.contains("\"meta\""), "not xhgui body: {req}");
    assert!(
        !req.contains("\"$schema\""),
        "speedscope leaked despite active envelope: {req}"
    );
}
