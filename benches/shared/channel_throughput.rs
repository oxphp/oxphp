//! Perf gate for Channel batched ops.
//!
//! The batched `recvMany(10)` path must be at least ~2x faster than 10
//! individual `recv()` calls. The crossing point between "always faster"
//! and "meaningful headroom" lives at the crossbeam poll-quantum (each
//! `recv_blocking` acquires `rx` once), so this bench drives a
//! pre-filled buffer and measures steady-state drain cost without any
//! blocking waits.
//!
//! The harness only produces numbers; the 2x gate is checked manually
//! against `target/criterion/*` summaries.

use criterion::{criterion_group, criterion_main, Criterion};
use oxphp::plugins::ox_shared::types::channel::{ChannelInner, Payload};
use oxphp::plugins::ox_shared::types::timeout::Wait;
use std::sync::Arc;

fn bench_recv_single_vs_batched(c: &mut Criterion) {
    let ch = Arc::new(ChannelInner::new(1_000));

    let mut group = c.benchmark_group("channel_recv_10");

    // 10 individual try_recv calls on a pre-filled buffer.
    group.bench_function("10x_single", |b| {
        b.iter(|| {
            // Top up so the next drain has exactly 10 items.
            for i in 0..10u8 {
                let _ = ch.try_send(Payload::bytes_only(vec![i]));
            }
            for _ in 0..10 {
                let _ = ch.try_recv();
            }
        });
    });

    // Single recv_many(10, Forever) call. The buffer is already topped
    // up to 10 so no actual blocking happens — Wait::Forever just means
    // "no deadline", and recv_many returns as soon as the 10th item is
    // received.
    group.bench_function("batched_recv_many", |b| {
        b.iter(|| {
            for i in 0..10u8 {
                let _ = ch.try_send(Payload::bytes_only(vec![i]));
            }
            let _ = ch.recv_many(10, Wait::Forever);
        });
    });

    group.finish();
}

/// SPSC throughput baseline — measures steady-state cost of
/// send+recv for 10_000 small items on a pre-sized channel, driven
/// from a single thread so the result captures pure per-op overhead
/// (no lock contention). Extrapolate to 1M via Criterion's throughput
/// annotation on this group.
fn bench_spsc_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("channel_spsc_throughput");
    group.throughput(criterion::Throughput::Elements(10_000));
    group.bench_function("send_recv_10k", |b| {
        let ch = ChannelInner::new(10_000);
        b.iter(|| {
            for i in 0..10_000u16 {
                let _ = ch.try_send(Payload::bytes_only(i.to_le_bytes().to_vec()));
            }
            for _ in 0..10_000 {
                let _ = ch.try_recv();
            }
        });
    });
    group.finish();
}

criterion_group!(benches, bench_recv_single_vs_batched, bench_spsc_throughput);
criterion_main!(benches);
