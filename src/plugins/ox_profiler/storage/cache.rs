//! In-memory LRU cache of recent `SpanTree`s. Used by the internal
//! routes to re-export profiles without re-reading from disk.
//! Holds `Arc<SpanTree>` so callers can read concurrently without
//! holding the cache lock.

use std::num::NonZeroUsize;
use std::sync::Arc;

use lru::LruCache;
use parking_lot::RwLock;

use crate::profiling::SpanTree;

pub struct ProfileCache {
    inner: RwLock<LruCache<String, Arc<SpanTree>>>,
}

impl ProfileCache {
    pub fn new(capacity: usize) -> Self {
        let cap = NonZeroUsize::new(capacity.max(1)).expect("non-zero");
        Self {
            inner: RwLock::new(LruCache::new(cap)),
        }
    }

    pub fn put(&self, run_id: String, tree: Arc<SpanTree>) {
        self.inner.write().put(run_id, tree);
    }

    pub fn get(&self, run_id: &str) -> Option<Arc<SpanTree>> {
        // `peek` does not promote recency, so the read path takes the
        // read lock and runs concurrently with other readers. The
        // write-side (`put`) drives eviction ordering, which is
        // acceptable for a profile cache where writes are the signal.
        self.inner.read().peek(run_id).cloned()
    }

    /// Number of cached entries (gauge for `oxphp_profiler_in_memory_runs`).
    pub fn len(&self) -> usize {
        self.inner.read().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profiling::ProfilingMode;

    fn empty_tree() -> Arc<SpanTree> {
        Arc::new(SpanTree {
            finished: vec![],
            trace_id: "t".into(),
            root_span_id: "r".into(),
            mode: ProfilingMode::ProfileAll,
        })
    }

    #[test]
    fn put_and_get_round_trip() {
        let c = ProfileCache::new(3);
        let t = empty_tree();
        c.put("run-1".into(), Arc::clone(&t));
        let got = c.get("run-1").expect("present");
        assert!(Arc::ptr_eq(&t, &got));
    }

    #[test]
    fn miss_returns_none() {
        let c = ProfileCache::new(3);
        assert!(c.get("missing").is_none());
    }

    #[test]
    fn capacity_eviction_drops_oldest() {
        let c = ProfileCache::new(2);
        c.put("a".into(), empty_tree());
        c.put("b".into(), empty_tree());
        c.put("c".into(), empty_tree());
        assert!(c.get("a").is_none(), "oldest evicted");
        assert!(c.get("b").is_some());
        assert!(c.get("c").is_some());
    }

    #[test]
    fn zero_capacity_normalises_to_one() {
        let c = ProfileCache::new(0);
        c.put("a".into(), empty_tree());
        assert!(c.get("a").is_some());
    }
}
