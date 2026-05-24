//! Fiber-suspending recv round-trip bench.
//! Measures: synthetic-promise alloc -> channel send -> promise resolve -> await.
//! Target: <= 20us p50.
//!
//! The scenario stands in for the PHP-level `$ch->recv(timeout: N)` path:
//! a worker fiber allocates a synthetic promise and parks it on
//! `register_recv_waiter`, then a producer on another tokio task fires
//! `try_send`, which hands the payload straight to the parked waiter and
//! resolves the promise. The cost measured here is the pure Rust half of
//! the round trip — fiber wake-up on the PHP side is captured by the
//! synthetic-promise bench.
//!
//! Harness produces numbers; the 20us gate is checked manually against
//! `target/criterion/*` summaries.

use criterion::{criterion_group, criterion_main, Criterion};
use oxphp::plugins::ox_shared::types::channel::{ChannelInner, Payload};
use std::sync::Arc;

fn bench_recv_via_synthetic_roundtrip(c: &mut Criterion) {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("tokio runtime");

    c.bench_function("channel_recv_fiber_roundtrip", |b| {
        b.to_async(&rt).iter(|| async {
            let ch = Arc::new(ChannelInner::new(4));
            let (id, rx) = oxphp::plugins::ox_async::synthetic::alloc();

            // Park a recv waiter on the channel. `try_send` from the
            // producer task will hand the payload straight to this id,
            // resolving `rx` without the payload ever landing in the
            // crossbeam buffer.
            ch.register_recv_waiter(id);

            // Deliver from another tokio task (simulates cross-thread send).
            let ch2 = ch.clone();
            let sender = tokio::spawn(async move {
                // Yield once to ensure the parker has parked.
                tokio::task::yield_now().await;
                ch2.try_send(Payload::bytes_only(vec![1, 2, 3, 4])).unwrap();
            });

            let result = rx.await.expect("payload received");
            sender.await.expect("sender task");
            assert!(result.success);
            assert_eq!(result.serialized_value_len, 4);
        });
    });
}

criterion_group!(benches, bench_recv_via_synthetic_roundtrip);
criterion_main!(benches);
