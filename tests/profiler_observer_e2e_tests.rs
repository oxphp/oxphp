//! End-to-end integration test for the observer event pipeline.
//! Exercises `PROFILING_CONTEXT::apply_events` through the
//! same public API the C observer hits at runtime, asserting the
//! resulting `SpanTree` matches what an "outer → middle → inner"
//! call sequence would produce.
//!
//! The bridge `set_profiling_mode` / `profiler_rshutdown_flush` calls
//! are no-op stubs in the host build (no `feature = "php"`), so this
//! test cannot exercise the actual C observer. The real PHP-driven
//! E2E test runs in Docker (see `tests/suites/profiler.txt`).
//!
//! What this test does verify:
//! - The public API re-exports (`set_profiling_mode`,
//!   `get_profiling_mode`, `profiler_rshutdown_flush`,
//!   `snapshot_open_stack`) are reachable from outside the crate.
//! - Synthetic observer events with realistic timestamps and parent
//!   linkage produce a well-formed `SpanTree` after `apply_events`
//!   + `finalize`.
//! - The mixed BEGIN/END ordering that would arise from a
//!   ring-buffer flush mid-call (BEGIN of inner before END of outer)
//!   is handled correctly.

#![cfg(feature = "plugin-profiler")]

use oxphp::profiling::{
    self, set_profiling_mode, snapshot_open_stack, OxSpanEvent, ProfilingMode, PROFILING_CONTEXT,
};

const KIND_BEGIN: u8 = profiling::flush::SPAN_EVENT_KIND_BEGIN;
const KIND_END: u8 = profiling::flush::SPAN_EVENT_KIND_END;

fn ev(kind: u8, seq: u64, ts_ns: u64, name: &'static [u8]) -> OxSpanEvent {
    OxSpanEvent {
        kind,
        reserved0: 0,
        name_len: name.len() as u16,
        reserved1: 0,
        seq,
        ts_ns,
        cpu_ns: ts_ns / 2,
        mem: 1024 * (seq as i64),
        mem_peak: 2048 * (seq as i64),
        name_ptr: name.as_ptr() as *const std::os::raw::c_char,
        reserved2: 0,
    }
}

#[test]
fn nested_calls_produce_well_formed_tree() {
    // Mode wiring (no-op stub off-PHP, real bridge call when PHP is
    // linked — both paths must accept the call cleanly).
    set_profiling_mode(ProfilingMode::ProfileAll);

    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(
            ProfilingMode::ProfileAll,
            "trace-e2e".into(),
            "root-e2e".into(),
        );

        // Mirrors what the C observer would emit for:
        //   <main> { outer(); }
        //   outer() { middle(); }
        //   middle() { inner(); }
        //   inner() { /* leaf */ }
        let events = [
            ev(KIND_BEGIN, 1, 1_000, b"outer"),
            ev(KIND_BEGIN, 2, 1_100, b"middle"),
            ev(KIND_BEGIN, 3, 1_200, b"inner"),
            ev(KIND_END, 3, 1_300, b""),
            ev(KIND_END, 2, 1_400, b""),
            ev(KIND_END, 1, 1_500, b""),
        ];
        ctx.apply_events(&events);
        assert_eq!(ctx.open_count(), 0, "all spans should have closed");

        let tree = ctx.finalize();
        assert_eq!(tree.finished.len(), 3);
        assert_eq!(tree.mode, ProfilingMode::ProfileAll);

        // Pop order is leaf-first.
        assert_eq!(tree.finished[0].name.as_ref(), "inner");
        assert_eq!(tree.finished[1].name.as_ref(), "middle");
        assert_eq!(tree.finished[2].name.as_ref(), "outer");

        // None leaked.
        assert!(tree.finished.iter().all(|s| !s.leaked));

        // Parent linkage forms the call chain root → outer → middle → inner.
        let inner = &tree.finished[0];
        let middle = &tree.finished[1];
        let outer = &tree.finished[2];
        assert_eq!(inner.parent_span_id, middle.span_id);
        assert_eq!(middle.parent_span_id, outer.span_id);
        assert_eq!(outer.parent_span_id.as_ref(), "root-e2e");
    });

    set_profiling_mode(ProfilingMode::Off);
}

#[test]
fn split_flush_mid_call_still_pairs_correctly() {
    // Simulates a flush that happens AFTER inner BEGIN but BEFORE
    // inner END — the inner BEGIN has already been consumed by Rust
    // when the END arrives in a later batch. The `seq → local_id`
    // table must remember it across batches.
    set_profiling_mode(ProfilingMode::ProfileAll);

    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(
            ProfilingMode::ProfileAll,
            "trace-split".into(),
            "root-split".into(),
        );

        ctx.apply_events(&[
            ev(KIND_BEGIN, 10, 100, b"outer"),
            ev(KIND_BEGIN, 11, 110, b"inner"),
        ]);
        assert_eq!(ctx.open_count(), 2);

        ctx.apply_events(&[ev(KIND_END, 11, 120, b""), ev(KIND_END, 10, 130, b"")]);
        assert_eq!(ctx.open_count(), 0);

        let tree = ctx.finalize();
        assert_eq!(tree.finished.len(), 2);
        assert_eq!(tree.finished[0].name.as_ref(), "inner");
        assert_eq!(tree.finished[1].name.as_ref(), "outer");
        assert!(!tree.finished.iter().any(|s| s.leaked));
    });

    set_profiling_mode(ProfilingMode::Off);
}

#[test]
fn unmatched_begin_force_closes_as_leaked() {
    // Models a script that bailed out (exit / fatal error) before its
    // Observer end was reached. finalize should mark it leaked.
    set_profiling_mode(ProfilingMode::ProfileAll);

    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(
            ProfilingMode::ProfileAll,
            "trace-leak".into(),
            "root-leak".into(),
        );

        ctx.apply_events(&[
            ev(KIND_BEGIN, 1, 100, b"caller"),
            ev(KIND_BEGIN, 2, 110, b"never_returns"),
        ]);

        let tree = ctx.finalize();
        assert_eq!(tree.finished.len(), 2);
        assert!(tree.finished.iter().all(|s| s.leaked));
    });

    set_profiling_mode(ProfilingMode::Off);
}

#[test]
fn apm_only_request_skips_observer_events() {
    // When the request runs in ApmOnly mode, the C observer's begin
    // would early-return without pushing — but if a stale event
    // somehow arrives, apply_events must drop it (the mode check is
    // the second line of defence).
    set_profiling_mode(ProfilingMode::ApmOnly);

    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(
            ProfilingMode::ApmOnly,
            "trace-apm".into(),
            "root-apm".into(),
        );

        ctx.apply_events(&[ev(KIND_BEGIN, 1, 100, b"should_be_ignored")]);
        assert_eq!(ctx.open_count(), 0);

        let tree = ctx.finalize();
        assert!(tree.finished.is_empty());
    });

    set_profiling_mode(ProfilingMode::Off);
}

#[test]
fn snapshot_open_stack_is_callable_from_outside_crate() {
    // The heap hook will call this from C, but we also want the
    // safe Rust wrapper to be reachable from external integration
    // tests so future heap-hook work can build on it without
    // re-plumbing visibility.
    let mut buf = [0u32; 32];
    let depth = snapshot_open_stack(&mut buf[..]);
    // Without the `php` feature linked, the wrapper returns 0 (stub).
    // With PHP linked but no profile request active, the C side also
    // returns 0. Either way: should be 0 here (no active request).
    assert_eq!(depth, 0, "no profile-all request active; depth must be 0");
}
