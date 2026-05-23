//! Channel shutdown-drain semantics.
//! Verifies that `SharedRegistry::drain()` wakes blocked send/recv ops
//! via `on_shutdown_notify` → `close()`.

#![cfg(feature = "plugin-shared")]

use std::sync::Arc;
use std::time::Duration;

use oxphp::plugins::ox_shared::config::{LockDiagnosticsLevel, SharedConfig};
use oxphp::plugins::ox_shared::registry::{self, SharedType};
use oxphp::plugins::ox_shared::types::channel::ChannelInner;
use oxphp::plugins::ox_shared::types::timeout::Wait;

// Helper: ensure the process-global registry is initialised. Multiple
// tests in this integration binary share it (OnceLock), so we call
// `init_registry` unconditionally — its internal `set(..).ok()` silently
// ignores the re-init attempt when already set, keeping whatever config
// was first installed.
fn ensure_registry_initialised() {
    registry::init_registry(SharedConfig {
        enabled: true,
        max_entries: 1_000,
        max_bytes: 64 * 1024,
        soft_limit_ratio: 0.9,
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

#[test]
fn blocked_recv_wakes_on_drain_with_none() {
    ensure_registry_initialised();
    let reg = registry::registry();

    let ch_inner = Arc::new(ChannelInner::new(4));
    let entry = reg
        .insert(SharedType::Channel, ch_inner.clone())
        .expect("insert");

    let ch_for_thread = ch_inner.clone();
    let handle = std::thread::spawn(move || {
        // Block for up to 5 seconds; drain should fire within the
        // receiver's POLL_QUANTUM (~20ms).
        ch_for_thread.recv_blocking(Wait::Bounded(Duration::from_secs(5)))
    });

    // Let the receiver arm (enter its poll loop).
    std::thread::sleep(Duration::from_millis(50));

    // Drain: fires on_shutdown_notify → close() on every entry.
    let wake_start = std::time::Instant::now();
    reg.drain();

    let result = handle.join().expect("thread joined");
    let wake_latency = wake_start.elapsed();

    // Must return Ok(None) — closed + empty.
    match result {
        Ok(None) => {} // success
        other => panic!("expected Ok(None) after drain, got {other:?}"),
    }
    // Wake latency budget: POLL_QUANTUM is 20ms; allow scheduler tail.
    // Empirically should be ~20–60ms. 250ms is a comfortable ceiling.
    assert!(
        wake_latency < Duration::from_millis(250),
        "wake_latency {wake_latency:?} exceeds 250ms — drain did not propagate promptly"
    );

    // Cleanup so the registry state doesn't bleed into sibling tests.
    // Dropping the last Arc<Entry> self-deregisters via Entry::Drop.
    drop(entry);
}

#[test]
fn blocked_send_wakes_on_drain_with_closed() {
    ensure_registry_initialised();
    let reg = registry::registry();

    let ch_inner = Arc::new(ChannelInner::new(1));
    // Fill the channel so the next send blocks.
    ch_inner.try_send(vec![1]).expect("initial send");

    let entry = reg
        .insert(SharedType::Channel, ch_inner.clone())
        .expect("insert");

    let ch_for_thread = ch_inner.clone();
    let handle = std::thread::spawn(move || {
        // Will block because the channel is full.
        ch_for_thread.send_blocking(vec![2], Wait::Bounded(Duration::from_secs(5)))
    });

    std::thread::sleep(Duration::from_millis(50));

    let wake_start = std::time::Instant::now();
    reg.drain();

    let result = handle.join().expect("thread joined");
    let wake_latency = wake_start.elapsed();

    use oxphp::plugins::ox_shared::error::SharedError;
    match result {
        Err(SharedError::Closed) => {} // success
        other => panic!("expected Err(Closed) after drain, got {other:?}"),
    }
    assert!(
        wake_latency < Duration::from_millis(250),
        "wake_latency {wake_latency:?} exceeds 250ms"
    );

    drop(entry);
}
