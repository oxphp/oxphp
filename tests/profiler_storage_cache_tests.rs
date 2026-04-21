//! Integration tests for ProfileCache.

#![cfg(feature = "plugin-profiler")]

use std::sync::Arc;

use oxphp::plugins::ox_profiler::storage::ProfileCache;
use oxphp::profiling::{ProfilingMode, SpanTree};

fn empty_tree() -> Arc<SpanTree> {
    Arc::new(SpanTree {
        finished: vec![],
        trace_id: "t".into(),
        root_span_id: "r".into(),
        mode: ProfilingMode::ProfileAll,
    })
}

#[test]
fn cache_round_trip() {
    let c = ProfileCache::new(8);
    let t = empty_tree();
    c.put("run-1".into(), Arc::clone(&t));
    let got = c.get("run-1").unwrap();
    assert!(Arc::ptr_eq(&t, &got));
}

#[test]
fn cache_concurrent_access() {
    use std::thread;
    let c = Arc::new(ProfileCache::new(64));
    let mut handles = vec![];
    for i in 0..16 {
        let c = Arc::clone(&c);
        handles.push(thread::spawn(move || {
            for j in 0..10 {
                let id = format!("run-{i}-{j}");
                c.put(id.clone(), empty_tree());
                assert!(c.get(&id).is_some());
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert!(c.len() <= 64);
}
