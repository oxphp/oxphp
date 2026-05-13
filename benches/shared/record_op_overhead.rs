//! Measures the overhead of `SharedRegistry::record_op` and the
//! second DashMap lookup it performs on the hot path of `Shared\*`
//! primitive operations.
//!
//! Four variants per type are benchmarked:
//!   V0 bare        — std primitive without registry indirection
//!   V1 current     — actual FFI entrypoint (ships today)
//!   V2 one_lookup  — registry.lookup() reused, manual ops.fetch_add
//!   V3 no_record_op — V2 minus the ops.fetch_add
//!
//! Each variant is measured single-thread (criterion bench_function)
//! and multi-thread at N=4, N=8 (iter_custom + Barrier).

use criterion::{criterion_group, criterion_main, Criterion};
use oxphp::plugins::ox_shared::config::{LockDiagnosticsLevel, SharedConfig};
use oxphp::plugins::ox_shared::registry::init_registry;

fn ensure_registry() {
    // metrics_enabled is set to false here even though it does not
    // currently gate record_op — that gap is part of what the bench
    // measures. introspection_enabled is also false to avoid any
    // background sampling work interfering with measurements.
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
        shutdown_timeout_seconds: 5.0,
        poison_strict: false,
        lock_diagnostics: LockDiagnosticsLevel::Off,
        lock_poll_interval_ms: 100,
        preview_string_limit: 256,
        preview_array_limit: 20,
    });
}

fn bench_record_op_overhead(c: &mut Criterion) {
    ensure_registry();
    // Per-type groups are filled in by later tasks.
    let _ = c;
}

criterion_group!(benches, bench_record_op_overhead);
criterion_main!(benches);
