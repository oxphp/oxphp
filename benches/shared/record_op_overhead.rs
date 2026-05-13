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

use criterion::BenchmarkId;
use criterion::Criterion;
use oxphp::plugins::ox_shared::config::{LockDiagnosticsLevel, SharedConfig};
use oxphp::plugins::ox_shared::registry::init_registry;
use oxphp::plugins::ox_shared::registry::registry;
use oxphp::plugins::ox_shared::types::atomic::{
    oxphp_shared_atomic_create, oxphp_shared_atomic_load, AtomicInner, SharedInnerAtomicExt,
};
use oxphp::plugins::ox_shared::types::counter::{
    oxphp_shared_counter_add, oxphp_shared_counter_create, CounterInner, SharedInnerCounterExt,
};
use oxphp::plugins::ox_shared::types::flag::{
    oxphp_shared_flag_create, oxphp_shared_flag_test, FlagInner, SharedInnerFlagExt,
};
use oxphp::plugins::ox_shared::types::once::{
    oxphp_shared_once_create, oxphp_shared_once_is_initialized, OnceInner, SharedInnerOnceExt,
};
use std::sync::atomic::AtomicBool;
use std::sync::atomic::{AtomicI64, Ordering};

#[derive(Copy, Clone)]
#[allow(dead_code)] // fields consumed by per-type bench fns added in later tasks
struct EntryIds {
    atomic: u64,
    counter: u64,
    flag: u64,
    once: u64,
}

fn setup_entries() -> EntryIds {
    let mut atomic = 0u64;
    let mut counter = 0u64;
    let mut flag = 0u64;
    let mut once = 0u64;

    let rc = unsafe { oxphp_shared_atomic_create(0, &mut atomic) };
    assert_eq!(rc, 0, "atomic_create failed");
    let rc = unsafe { oxphp_shared_counter_create(0, &mut counter) };
    assert_eq!(rc, 0, "counter_create failed");
    let rc = unsafe { oxphp_shared_flag_create(0, &mut flag) };
    assert_eq!(rc, 0, "flag_create failed");
    let rc = unsafe { oxphp_shared_once_create(&mut once) };
    assert_eq!(rc, 0, "once_create failed");

    EntryIds {
        atomic,
        counter,
        flag,
        once,
    }
}

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

use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

/// Run `body` `iters` times on each of `n_threads` worker threads,
/// timing from the synchronised barrier release to the last join.
///
/// `body` must be `Send + Sync` and is invoked with no arguments inside
/// each worker's iteration loop.
#[allow(dead_code)] // consumed by multi-thread bench fns in later tasks
fn run_threads<F>(n_threads: usize, iters: u64, body: F) -> Duration
where
    F: Fn() + Send + Sync,
{
    thread::scope(|s| {
        let barrier = Arc::new(Barrier::new(n_threads + 1));
        let body_ref = &body;

        let handles: Vec<_> = (0..n_threads)
            .map(|_| {
                let barrier = Arc::clone(&barrier);
                s.spawn(move || {
                    barrier.wait();
                    for _ in 0..iters {
                        body_ref();
                    }
                })
            })
            .collect();

        // Release all workers simultaneously, then start the clock.
        barrier.wait();
        let start = Instant::now();
        for h in handles {
            h.join().expect("worker panicked");
        }
        start.elapsed()
    })
}

// `0u8` is the wire encoding of `Ordering::Relaxed` accepted by
// oxphp_shared_atomic_load (see `ordering_from_u8` in atomic.rs).
const ORDER_RELAXED: u8 = 0;

fn bench_atomic_single(c: &mut Criterion, ids: EntryIds) {
    let mut group = c.benchmark_group("atomic_load");
    let id = ids.atomic;
    let reg = registry();

    // V0 bare — a thread-local AtomicI64 with no registry involvement.
    let bare = AtomicI64::new(0);
    group.bench_function(BenchmarkId::new("bare", 1), |b| {
        b.iter(|| {
            criterion::black_box(bare.load(Ordering::Relaxed));
        });
    });

    // V1 current — the actual public FFI entrypoint.
    group.bench_function(BenchmarkId::new("current", 1), |b| {
        b.iter(|| {
            let mut out: i64 = 0;
            let rc = unsafe { oxphp_shared_atomic_load(id, ORDER_RELAXED, &mut out) };
            debug_assert_eq!(rc, 0);
            criterion::black_box(out);
        });
    });

    // V2 one_lookup — single registry.lookup, manual record_op via
    // the already-resolved Arc<Entry>.
    group.bench_function(BenchmarkId::new("one_lookup", 1), |b| {
        b.iter(|| {
            let entry = reg.lookup(id).expect("entry exists");
            let inner: &AtomicInner = entry.inner.as_any_atomic().expect("type matches");
            let v = inner.load(Ordering::Relaxed);
            entry.ops.fetch_add(1, Ordering::Relaxed);
            criterion::black_box(v);
        });
    });

    // V3 no_record_op — V2 minus the ops.fetch_add.
    group.bench_function(BenchmarkId::new("no_record_op", 1), |b| {
        b.iter(|| {
            let entry = reg.lookup(id).expect("entry exists");
            let inner: &AtomicInner = entry.inner.as_any_atomic().expect("type matches");
            let v = inner.load(Ordering::Relaxed);
            criterion::black_box(v);
        });
    });

    group.finish();
}

fn bench_atomic_multi(c: &mut Criterion, ids: EntryIds) {
    let mut group = c.benchmark_group("atomic_load");
    let id = ids.atomic;

    for &n in &[4usize, 8usize] {
        // V0 bare — shared AtomicI64 across threads.
        let bare = Arc::new(AtomicI64::new(0));
        group.bench_function(BenchmarkId::new("bare", n), |b| {
            b.iter_custom(|iters| {
                let bare = Arc::clone(&bare);
                run_threads(n, iters, move || {
                    criterion::black_box(bare.load(Ordering::Relaxed));
                })
            });
        });

        // V1 current
        group.bench_function(BenchmarkId::new("current", n), |b| {
            b.iter_custom(|iters| {
                run_threads(n, iters, || {
                    let mut out: i64 = 0;
                    let rc = unsafe { oxphp_shared_atomic_load(id, ORDER_RELAXED, &mut out) };
                    debug_assert_eq!(rc, 0);
                    criterion::black_box(out);
                })
            });
        });

        // V2 one_lookup
        group.bench_function(BenchmarkId::new("one_lookup", n), |b| {
            b.iter_custom(|iters| {
                run_threads(n, iters, || {
                    let reg = registry();
                    let entry = reg.lookup(id).expect("entry exists");
                    let inner: &AtomicInner = entry.inner.as_any_atomic().expect("type matches");
                    let v = inner.load(Ordering::Relaxed);
                    entry.ops.fetch_add(1, Ordering::Relaxed);
                    criterion::black_box(v);
                })
            });
        });

        // V3 no_record_op
        group.bench_function(BenchmarkId::new("no_record_op", n), |b| {
            b.iter_custom(|iters| {
                run_threads(n, iters, || {
                    let reg = registry();
                    let entry = reg.lookup(id).expect("entry exists");
                    let inner: &AtomicInner = entry.inner.as_any_atomic().expect("type matches");
                    let v = inner.load(Ordering::Relaxed);
                    criterion::black_box(v);
                })
            });
        });
    }

    group.finish();
}

fn bench_counter(c: &mut Criterion, ids: EntryIds) {
    let mut group = c.benchmark_group("counter_add");
    let id = ids.counter;

    // V0 bare
    let bare_single = AtomicI64::new(0);
    group.bench_function(BenchmarkId::new("bare", 1), |b| {
        b.iter(|| {
            criterion::black_box(bare_single.fetch_add(1, Ordering::SeqCst));
        });
    });

    // V1 current
    group.bench_function(BenchmarkId::new("current", 1), |b| {
        b.iter(|| {
            let mut out: i64 = 0;
            let rc = unsafe { oxphp_shared_counter_add(id, 1, &mut out) };
            debug_assert_eq!(rc, 0);
            criterion::black_box(out);
        });
    });

    // V2 one_lookup
    {
        let reg = registry();
        group.bench_function(BenchmarkId::new("one_lookup", 1), |b| {
            b.iter(|| {
                let entry = reg.lookup(id).expect("entry exists");
                let inner: &CounterInner = entry.inner.as_any_counter().expect("type matches");
                let v = inner.add(1);
                entry.ops.fetch_add(1, Ordering::Relaxed);
                criterion::black_box(v);
            });
        });
    }

    // V3 no_record_op
    {
        let reg = registry();
        group.bench_function(BenchmarkId::new("no_record_op", 1), |b| {
            b.iter(|| {
                let entry = reg.lookup(id).expect("entry exists");
                let inner: &CounterInner = entry.inner.as_any_counter().expect("type matches");
                let v = inner.add(1);
                criterion::black_box(v);
            });
        });
    }

    for &n in &[4usize, 8usize] {
        let bare_multi = Arc::new(AtomicI64::new(0));
        group.bench_function(BenchmarkId::new("bare", n), |b| {
            b.iter_custom(|iters| {
                let bare = Arc::clone(&bare_multi);
                run_threads(n, iters, move || {
                    criterion::black_box(bare.fetch_add(1, Ordering::SeqCst));
                })
            });
        });
        group.bench_function(BenchmarkId::new("current", n), |b| {
            b.iter_custom(|iters| {
                run_threads(n, iters, || {
                    let mut out: i64 = 0;
                    let rc = unsafe { oxphp_shared_counter_add(id, 1, &mut out) };
                    debug_assert_eq!(rc, 0);
                    criterion::black_box(out);
                })
            });
        });
        group.bench_function(BenchmarkId::new("one_lookup", n), |b| {
            b.iter_custom(|iters| {
                run_threads(n, iters, || {
                    let reg = registry();
                    let entry = reg.lookup(id).expect("entry exists");
                    let inner: &CounterInner = entry.inner.as_any_counter().expect("type matches");
                    let v = inner.add(1);
                    entry.ops.fetch_add(1, Ordering::Relaxed);
                    criterion::black_box(v);
                })
            });
        });
        group.bench_function(BenchmarkId::new("no_record_op", n), |b| {
            b.iter_custom(|iters| {
                run_threads(n, iters, || {
                    let reg = registry();
                    let entry = reg.lookup(id).expect("entry exists");
                    let inner: &CounterInner = entry.inner.as_any_counter().expect("type matches");
                    let v = inner.add(1);
                    criterion::black_box(v);
                })
            });
        });
    }

    group.finish();
}

fn bench_flag(c: &mut Criterion, ids: EntryIds) {
    let mut group = c.benchmark_group("flag_test");
    let id = ids.flag;

    let bare_single = AtomicBool::new(false);
    group.bench_function(BenchmarkId::new("bare", 1), |b| {
        b.iter(|| {
            criterion::black_box(bare_single.load(Ordering::SeqCst));
        });
    });

    group.bench_function(BenchmarkId::new("current", 1), |b| {
        b.iter(|| {
            let mut out: std::os::raw::c_int = 0;
            let rc = unsafe { oxphp_shared_flag_test(id, &mut out) };
            debug_assert_eq!(rc, 0);
            criterion::black_box(out);
        });
    });

    {
        let reg = registry();
        group.bench_function(BenchmarkId::new("one_lookup", 1), |b| {
            b.iter(|| {
                let entry = reg.lookup(id).expect("entry exists");
                let inner: &FlagInner = entry.inner.as_any_flag().expect("type matches");
                let v = inner.test();
                entry.ops.fetch_add(1, Ordering::Relaxed);
                criterion::black_box(v);
            });
        });
    }

    {
        let reg = registry();
        group.bench_function(BenchmarkId::new("no_record_op", 1), |b| {
            b.iter(|| {
                let entry = reg.lookup(id).expect("entry exists");
                let inner: &FlagInner = entry.inner.as_any_flag().expect("type matches");
                let v = inner.test();
                criterion::black_box(v);
            });
        });
    }

    for &n in &[4usize, 8usize] {
        let bare_multi = Arc::new(AtomicBool::new(false));
        group.bench_function(BenchmarkId::new("bare", n), |b| {
            b.iter_custom(|iters| {
                let bare = Arc::clone(&bare_multi);
                run_threads(n, iters, move || {
                    criterion::black_box(bare.load(Ordering::SeqCst));
                })
            });
        });
        group.bench_function(BenchmarkId::new("current", n), |b| {
            b.iter_custom(|iters| {
                run_threads(n, iters, || {
                    let mut out: std::os::raw::c_int = 0;
                    let rc = unsafe { oxphp_shared_flag_test(id, &mut out) };
                    debug_assert_eq!(rc, 0);
                    criterion::black_box(out);
                })
            });
        });
        group.bench_function(BenchmarkId::new("one_lookup", n), |b| {
            b.iter_custom(|iters| {
                run_threads(n, iters, || {
                    let reg = registry();
                    let entry = reg.lookup(id).expect("entry exists");
                    let inner: &FlagInner = entry.inner.as_any_flag().expect("type matches");
                    let v = inner.test();
                    entry.ops.fetch_add(1, Ordering::Relaxed);
                    criterion::black_box(v);
                })
            });
        });
        group.bench_function(BenchmarkId::new("no_record_op", n), |b| {
            b.iter_custom(|iters| {
                run_threads(n, iters, || {
                    let reg = registry();
                    let entry = reg.lookup(id).expect("entry exists");
                    let inner: &FlagInner = entry.inner.as_any_flag().expect("type matches");
                    let v = inner.test();
                    criterion::black_box(v);
                })
            });
        });
    }

    group.finish();
}

fn bench_once(c: &mut Criterion, ids: EntryIds) {
    let mut group = c.benchmark_group("once_is_initialized");
    let id = ids.once;

    let bare_single = AtomicBool::new(false);
    group.bench_function(BenchmarkId::new("bare", 1), |b| {
        b.iter(|| {
            criterion::black_box(bare_single.load(Ordering::SeqCst));
        });
    });

    group.bench_function(BenchmarkId::new("current", 1), |b| {
        b.iter(|| {
            let mut out: std::os::raw::c_int = 0;
            let rc = unsafe { oxphp_shared_once_is_initialized(id, &mut out) };
            debug_assert_eq!(rc, 0);
            criterion::black_box(out);
        });
    });

    {
        let reg = registry();
        group.bench_function(BenchmarkId::new("one_lookup", 1), |b| {
            b.iter(|| {
                let entry = reg.lookup(id).expect("entry exists");
                let inner: &OnceInner = entry.inner.as_any_once().expect("type matches");
                let v = inner.is_initialized();
                entry.ops.fetch_add(1, Ordering::Relaxed);
                criterion::black_box(v);
            });
        });
    }

    {
        let reg = registry();
        group.bench_function(BenchmarkId::new("no_record_op", 1), |b| {
            b.iter(|| {
                let entry = reg.lookup(id).expect("entry exists");
                let inner: &OnceInner = entry.inner.as_any_once().expect("type matches");
                let v = inner.is_initialized();
                criterion::black_box(v);
            });
        });
    }

    for &n in &[4usize, 8usize] {
        let bare_multi = Arc::new(AtomicBool::new(false));
        group.bench_function(BenchmarkId::new("bare", n), |b| {
            b.iter_custom(|iters| {
                let bare = Arc::clone(&bare_multi);
                run_threads(n, iters, move || {
                    criterion::black_box(bare.load(Ordering::SeqCst));
                })
            });
        });
        group.bench_function(BenchmarkId::new("current", n), |b| {
            b.iter_custom(|iters| {
                run_threads(n, iters, || {
                    let mut out: std::os::raw::c_int = 0;
                    let rc = unsafe { oxphp_shared_once_is_initialized(id, &mut out) };
                    debug_assert_eq!(rc, 0);
                    criterion::black_box(out);
                })
            });
        });
        group.bench_function(BenchmarkId::new("one_lookup", n), |b| {
            b.iter_custom(|iters| {
                run_threads(n, iters, || {
                    let reg = registry();
                    let entry = reg.lookup(id).expect("entry exists");
                    let inner: &OnceInner = entry.inner.as_any_once().expect("type matches");
                    let v = inner.is_initialized();
                    entry.ops.fetch_add(1, Ordering::Relaxed);
                    criterion::black_box(v);
                })
            });
        });
        group.bench_function(BenchmarkId::new("no_record_op", n), |b| {
            b.iter_custom(|iters| {
                run_threads(n, iters, || {
                    let reg = registry();
                    let entry = reg.lookup(id).expect("entry exists");
                    let inner: &OnceInner = entry.inner.as_any_once().expect("type matches");
                    let v = inner.is_initialized();
                    criterion::black_box(v);
                })
            });
        });
    }

    group.finish();
}

fn bench_record_op_overhead(c: &mut Criterion) {
    ensure_registry();
    let ids = setup_entries();
    bench_atomic_single(c, ids);
    bench_atomic_multi(c, ids);
    bench_counter(c, ids);
    bench_flag(c, ids);
    bench_once(c, ids);
}

// ─── Summary table ────────────────────────────────────────────────────

use serde_json::Value;
use std::fs;
use std::path::PathBuf;

/// Reads the mean point-estimate (in ns) from a criterion sub-bench's
/// `estimates.json`. Returns None if the file is missing or malformed.
fn read_mean_ns(group: &str, variant: &str, threads: usize) -> Option<f64> {
    let path: PathBuf = [
        "target",
        "criterion",
        group,
        &format!("{variant}/{threads}"),
        "new",
        "estimates.json",
    ]
    .iter()
    .collect();
    let raw = fs::read_to_string(&path).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    v.get("mean")?.get("point_estimate")?.as_f64()
}

fn fmt_ns(opt: Option<f64>) -> String {
    match opt {
        Some(ns) if ns < 10.0 => format!("{ns:>6.2}ns"),
        Some(ns) if ns < 1_000.0 => format!("{ns:>6.1}ns"),
        Some(ns) if ns < 1_000_000.0 => format!("{:>6.1}µs", ns / 1_000.0),
        Some(ns) => format!("{:>6.1}ms", ns / 1_000_000.0),
        None => "    -- ".to_string(),
    }
}

fn fmt_ratio(numer: Option<f64>, denom: Option<f64>) -> String {
    match (numer, denom) {
        (Some(n), Some(d)) if d > 0.0 => format!("{:>5.1}x", n / d),
        _ => "  --x".to_string(),
    }
}

fn geomean(xs: &[f64]) -> Option<f64> {
    let nonzero: Vec<_> = xs.iter().copied().filter(|x| *x > 0.0).collect();
    if nonzero.is_empty() {
        return None;
    }
    let log_sum: f64 = nonzero.iter().map(|x| x.ln()).sum();
    Some((log_sum / nonzero.len() as f64).exp())
}

const GROUPS: &[&str] = &[
    "atomic_load",
    "counter_add",
    "flag_test",
    "once_is_initialized",
];
const VARIANTS: &[&str] = &["bare", "current", "one_lookup", "no_record_op"];

fn print_summary() {
    let host_cpus = std::thread::available_parallelism()
        .map(|n| n.get().to_string())
        .unwrap_or_else(|_| "?".into());
    println!();
    println!("=== record_op overhead summary (host: {host_cpus} cores) ===");

    let mut current_vs_bare_8t = Vec::new();
    let mut current_vs_no_record_op_8t = Vec::new();
    let mut v2_vs_v3_8t = Vec::new();

    for group in GROUPS {
        println!();
        println!("{group:>22}    1t       4t       8t");
        for variant in VARIANTS {
            let t1 = read_mean_ns(group, variant, 1);
            let t4 = read_mean_ns(group, variant, 4);
            let t8 = read_mean_ns(group, variant, 8);
            let ratio_str = if *variant == "bare" {
                String::new()
            } else {
                let bare_t8 = read_mean_ns(group, "bare", 8);
                format!("   ratio_8t={}", fmt_ratio(t8, bare_t8))
            };
            println!(
                "  {variant:<16} {} {} {}{}",
                fmt_ns(t1),
                fmt_ns(t4),
                fmt_ns(t8),
                ratio_str
            );
        }

        let bare_8t = read_mean_ns(group, "bare", 8);
        let current_8t = read_mean_ns(group, "current", 8);
        let one_lookup_8t = read_mean_ns(group, "one_lookup", 8);
        let no_record_op_8t = read_mean_ns(group, "no_record_op", 8);

        if let (Some(c), Some(b)) = (current_8t, bare_8t) {
            if b > 0.0 {
                current_vs_bare_8t.push(c / b);
            }
        }
        if let (Some(c), Some(nro)) = (current_8t, no_record_op_8t) {
            if nro > 0.0 {
                current_vs_no_record_op_8t.push(c / nro);
            }
        }
        if let (Some(v2), Some(v3)) = (one_lookup_8t, no_record_op_8t) {
            if v3 > 0.0 {
                v2_vs_v3_8t.push(v2 / v3);
            }
        }
    }

    println!();
    println!("DECISION INPUTS (geomean across types at 8 threads):");
    println!(
        "  current vs bare:            {}",
        fmt_ratio(geomean(&current_vs_bare_8t), Some(1.0))
    );
    println!(
        "  current vs no_record_op:    {}   <- record_op-specific overhead",
        fmt_ratio(geomean(&current_vs_no_record_op_8t), Some(1.0))
    );
    println!(
        "  one_lookup vs no_record_op: {}   <- record_op overhead w/o 2nd lookup",
        fmt_ratio(geomean(&v2_vs_v3_8t), Some(1.0))
    );
    println!();
}

fn main() {
    let mut criterion = Criterion::default().configure_from_args();
    bench_record_op_overhead(&mut criterion);
    criterion.final_summary();
    print_summary();
}
