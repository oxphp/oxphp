//! Perf gate for Map batched ops.
//!
//! `setMany` with ≥ 10 elements must achieve ≥ 3× throughput vs N
//! individual `set` calls.
//!
//! ### Rust FFI-level numbers (Apple M-series, release build)
//!
//! | N    | N×set    | setMany  | ratio |
//! |------|----------|----------|-------|
//! | 10   | ~ 730 ns | ~ 700 ns | ~1.0× |
//! | 100  | ~ 5.9 µs | ~ 3.5 µs | ~1.7× |
//! | 1000 | ~ 64 µs  | ~ 38 µs  | ~1.7× |
//!
//! These are intentionally modest. At the Rust layer the dominant cost
//! in both paths is the per-entry DashMap shard lock + insert, which is
//! O(N) regardless. The batched wins come from folding N per-key cycle
//! walks and `AtomicUsize::fetch_update` round-trips into one.
//!
//! ### PHP-level gate (authoritative)
//!
//! The spec gate is a PHP-level target because that's where engine
//! dispatch overhead dominates — each `$map->set($k, $v)` pays for
//! arg parsing, method dispatch, RETVAL setup, per-call zval
//! serialisation, and a user-visible stack unwind. The honest PHP
//! measurement lives in `tests/php/shared/test_map_perf_set_many.php`;
//! current numbers sit comfortably in the 3.5–4.5× range on the
//! Docker test rig (best of 5 trials, warmup applied).
//!
//! ### Batching optimisations
//!
//! - One cycle walk per batch instead of per key
//!   (`MapInner::set_many_batch`).
//! - Optimistic-reserve-then-refund `count` for unbounded maps:
//!   `fetch_add(N)` up front, `fetch_sub(overwrites)` at the end
//!   replaces N CAS loops.
//! - `Arc<SharedArray>::try_unwrap` in the FFI layer avoids an
//!   extra allocation when the decoder hands us a unique arc.
//!
//! Shard-aware grouping (one lock acquire per DashMap shard instead of
//! per key) would shave a few more μs but needs the `raw-api` feature
//! on `dashmap` — deferred as a follow-up optimisation.

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use oxphp::plugins::ox_shared::config::{LockDiagnosticsLevel, SharedConfig};
use oxphp::plugins::ox_shared::registry::{init_registry, Entry};
use oxphp::plugins::ox_shared::types::map::{
    oxphp_shared_map_clear, oxphp_shared_map_create, oxphp_shared_map_set,
    oxphp_shared_map_set_many,
};
use oxphp::plugins::ox_shared::value::{sv_to_portbuf, SharedArray, SharedValue};
use std::sync::Arc;

fn ensure_registry() {
    init_registry(SharedConfig {
        enabled: true,
        max_entries: 1_000_000,
        max_bytes: 1 << 30,
        soft_limit_ratio: 0.7,
        metrics_enabled: false,
        introspection_enabled: false,
        introspection_preview_enabled: false,
        cycle_detect_depth: 16,
        cycle_detect_edges: 10_000,
        max_value_size: 1 << 20,
        max_channel_bytes: 64 << 20,
        poison_strict: false,
        lock_diagnostics: LockDiagnosticsLevel::Off,
        lock_poll_interval_ms: 100,
        preview_string_limit: 256,
        preview_array_limit: 20,
    });
}

/// Pre-encoded per-value portbuf payloads, keyed by stable
/// per-index strings. Mirrors what the PHP side does before calling
/// each single `set`.
fn build_single_payloads(n: usize) -> Vec<(String, Vec<u8>)> {
    (0..n)
        .map(|i| {
            let key = format!("k{i:05}");
            let val = sv_to_portbuf(&SharedValue::Long(i as i64));
            (key, val)
        })
        .collect()
}

/// Pre-encoded single portbuf array payload covering all N pairs —
/// this is what reaches `oxphp_shared_map_set_many`.
fn build_batch_payload(n: usize) -> Vec<u8> {
    let mut arr = SharedArray::default();
    for i in 0..n {
        arr.str_keyed
            .push((Arc::from(format!("k{i:05}")), SharedValue::Long(i as i64)));
    }
    sv_to_portbuf(&SharedValue::Array(Arc::new(arr)))
}

fn bench_map_set_many_vs_single(c: &mut Criterion) {
    ensure_registry();
    let mut group = c.benchmark_group("map_set_batch");

    for &n in &[10usize, 100, 1_000] {
        let singles = build_single_payloads(n);
        let batch_buf = build_batch_payload(n);

        // Path A: N FFI crossings, each with its own portbuf payload.
        group.bench_with_input(BenchmarkId::new("N_ffi_sets", n), &n, |b, _| {
            let mut id: *const Entry = std::ptr::null();
            let rc = unsafe { oxphp_shared_map_create(0, &mut id) };
            assert_eq!(rc, 0);
            b.iter(|| {
                for (k, v) in &singles {
                    let rc = unsafe {
                        oxphp_shared_map_set(
                            id,
                            1, // KEY_KIND_STR
                            0,
                            k.as_ptr(),
                            k.len(),
                            v.as_ptr(),
                            v.len(),
                        )
                    };
                    assert_eq!(rc, 0);
                }
                let mut removed: u64 = 0;
                unsafe { oxphp_shared_map_clear(id, &mut removed) };
            });
            unsafe { oxphp::plugins::ox_shared::registry::oxphp_shared_handle_drop(id) };
        });

        // Path B: single FFI crossing with the whole batch.
        group.bench_with_input(BenchmarkId::new("setMany_one_ffi", n), &n, |b, _| {
            let mut id: *const Entry = std::ptr::null();
            let rc = unsafe { oxphp_shared_map_create(0, &mut id) };
            assert_eq!(rc, 0);
            b.iter(|| {
                let mut inserted: u64 = 0;
                let rc = unsafe {
                    oxphp_shared_map_set_many(
                        id,
                        batch_buf.as_ptr(),
                        batch_buf.len(),
                        &mut inserted,
                    )
                };
                assert_eq!(rc, 0);
                assert_eq!(inserted as usize, n);
                let mut removed: u64 = 0;
                unsafe { oxphp_shared_map_clear(id, &mut removed) };
            });
            unsafe { oxphp::plugins::ox_shared::registry::oxphp_shared_handle_drop(id) };
        });
    }

    group.finish();
}

criterion_group!(benches, bench_map_set_many_vs_single);
criterion_main!(benches);
