//! Perf gate for synthetic-promise round-trip (Channel fiber integration).
//!
//! Measures one full synthetic-promise round-trip:
//!   alloc → cross-thread resolve → receiver.await → return.
//!
//! Target: p50 ≤ 20μs, p99 ≤ 50μs on commodity 8-core x86-64.
//! Failure would require pivoting to a native `fiber_suspend_on(waker)`
//! primitive before committing.

use criterion::{criterion_group, criterion_main, Criterion};
use oxphp::plugins::ox_async::synthetic::{alloc, resolve, PromisePayload};

fn bench_roundtrip(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");

    c.bench_function("synthetic_promise_roundtrip", |b| {
        b.to_async(&rt).iter(|| async {
            let (id, rx) = alloc();
            // Resolve from another tokio task (simulates cross-thread).
            let resolver = tokio::spawn(async move {
                let _ = resolve(id, PromisePayload::Value(vec![1, 2, 3, 4]));
            });
            let result = rx.await.expect("payload received");
            resolver.await.expect("resolver task");
            assert!(result.success);
            assert_eq!(result.serialized_value_len, 4);
            // AsyncResult::drop will free the malloc'd serialized_value.
        });
    });
}

criterion_group!(benches, bench_roundtrip);
criterion_main!(benches);
