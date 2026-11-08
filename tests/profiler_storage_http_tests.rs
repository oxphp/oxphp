//! Integration tests for HttpPusher.

#![cfg(feature = "plugin-profiler")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use oxphp::plugins::ox_profiler::storage::{HttpPusher, OutputFormat, RunMeta, StorageMetrics};
use oxphp::plugins::ox_profiler::trigger::ActivationSource;
use oxphp::profiling::{FinishedSpan, ProfilingMode, SpanTree};

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
        truncated: false,
    })
}

/// One profiled call, so an xhprof body has a pair to find at its top level.
fn one_span_tree() -> Arc<SpanTree> {
    Arc::new(SpanTree {
        finished: vec![FinishedSpan {
            local_id: 1,
            trace_id: "t".into(),
            span_id: "s1".into(),
            parent_span_id: "r".into(),
            name: "op".into(),
            start_ns: 0,
            end_ns: 1_000,
            attributes: vec![],
            events: vec![],
            status_code: 0,
            status_message: None,
            leaked: false,
            cpu_ns: 0,
            mem_enter: 0,
            mem_exit: 0,
            mem_peak: 0,
        }],
        trace_id: "t".into(),
        root_span_id: "r".into(),
        mode: ProfilingMode::ProfileAll,
        truncated: false,
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

/// A push is the bare pair map whatever the URL looks like: the path a
/// profile UI's importer listens on does not change the body.
#[tokio::test]
async fn http_push_xhprof_is_a_bare_pair_map() {
    let (port, _hits, captured) = spawn_mock_server().await;
    let pusher = Arc::new(
        HttpPusher::new(
            format!("http://127.0.0.1:{port}/run/import"),
            OutputFormat::Xhprof,
            None,
            StorageMetrics::new(),
        )
        .unwrap(),
    );
    pusher.push(&empty_meta(), &one_span_tree()).await;

    let req = captured.lock().await;
    let (_, body) = req.split_once("\r\n\r\n").expect("request has a body");
    let v: serde_json::Value = serde_json::from_str(body).expect("body is JSON");
    let map = v.as_object().expect("top level is an object");
    assert!(
        map.contains_key("main()==>op"),
        "pair map at the top level: {body}"
    );
    assert!(!map.contains_key("profile"), "no wrapper: {body}");
    assert!(!map.contains_key("meta"), "no wrapper: {body}");
}

#[tokio::test]
async fn http_push_content_type_follows_format() {
    for (format, expected) in [
        (OutputFormat::Xhprof, "content-type: application/json"),
        (OutputFormat::Collapsed, "content-type: text/plain"),
    ] {
        let (port, _hits, captured) = spawn_mock_server().await;
        let pusher = Arc::new(
            HttpPusher::new(
                format!("http://127.0.0.1:{port}/"),
                format,
                None,
                StorageMetrics::new(),
            )
            .unwrap(),
        );
        pusher.push(&empty_meta(), &empty_tree()).await;
        let req = captured.lock().await.to_lowercase();
        assert!(req.contains(expected), "{format:?}: {req}");
    }
}
