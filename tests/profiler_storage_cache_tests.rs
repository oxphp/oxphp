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

/// Capacity is deliberately larger than the 16×10 working set: with a smaller
/// cache another thread's `put` can evict an entry between this thread's `put`
/// and its read-back, which makes the assertion a coin flip rather than a
/// statement about thread safety. Eviction has its own deterministic coverage
/// in the unit tests next to `ProfileCache`.
#[test]
fn cache_concurrent_access() {
    use std::thread;
    let c = Arc::new(ProfileCache::new(256));
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
    assert_eq!(c.len(), 160, "every concurrent put must survive");
}
