//! Integration tests for the OxPHP\Profile\* PHP SDK.
//!
//! Pure-Rust simulation — exercises ProfilingContext via the public
//! API the SDK function handlers hit at runtime
//! (attach_mark_on_current, attach_metric_on_current, set/get
//! profiling_paused, set_profiling_mode). The actual PHP-side
//! execution is covered by the Docker profile in
//! tests/suites/profiler.txt.

#![cfg(feature = "plugin-profiler")]

use oxphp::profiling::{
    is_profiling_paused, set_profiling_mode, set_profiling_paused, ProfilingMode, SpanEventKind,
    PROFILING_CONTEXT,
};

#[test]
fn paused_flag_wrappers_are_callable() {
    // In the host build (no feature = "php"), the wrappers are no-op
    // stubs — set_profiling_paused does nothing and is_profiling_paused
    // always returns false. Real round-trip behaviour is exercised in
    // the Docker E2E tests under tests/php/profiler/. This test just
    // confirms the symbols resolve and accept the expected types.
    set_profiling_mode(ProfilingMode::Off);
    set_profiling_paused(true);
    set_profiling_paused(false);
    let _ = is_profiling_paused();
    set_profiling_mode(ProfilingMode::ProfileAll);
    set_profiling_mode(ProfilingMode::Off);
}

#[test]
fn mark_attaches_to_current_span() {
    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = ctx.push("op".into(), vec![]);
        ctx.attach_mark_on_current(
            "user_loaded".into(),
            vec![(std::sync::Arc::from("user_id"), std::sync::Arc::from("42"))],
        );
        let span = ctx.get_mut(id).expect("span open");
        assert_eq!(span.events.len(), 1);
        assert_eq!(span.events[0].name, "user_loaded");
        assert_eq!(span.events[0].kind, SpanEventKind::Mark);
        assert_eq!(span.events[0].attributes[0].0.as_ref(), "user_id");
        assert_eq!(span.events[0].attributes[0].1.as_ref(), "42");
    });
}

#[test]
fn metric_appends_dotted_attribute_with_natural_formatting() {
    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = ctx.push("query".into(), vec![]);
        ctx.attach_metric_on_current("rows", 1234.0);
        ctx.attach_metric_on_current("ratio", 0.875);
        let span = ctx.get_mut(id).expect("span open");
        assert!(span
            .attributes
            .iter()
            .any(|(k, v)| k.as_ref() == "metric.rows" && v.as_ref() == "1234"));
        assert!(span
            .attributes
            .iter()
            .any(|(k, v)| k.as_ref() == "metric.ratio" && v.as_ref() == "0.875"));
    });
}

#[test]
fn mark_and_metric_are_noop_without_open_span() {
    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        ctx.attach_mark_on_current("orphan_mark".into(), vec![]);
        ctx.attach_metric_on_current("orphan_metric", 1.0);
        assert_eq!(ctx.open_count(), 0);
        assert_eq!(ctx.finished_count(), 0);
    });
}

#[test]
fn open_spans_close_naturally_after_pause() {
    // Models PHP code that calls pause() between push() and pop()
    // — the Rust side must let the matching pop go through (the
    // C-end callback intentionally ignores the paused flag).
    set_profiling_mode(ProfilingMode::ProfileAll);
    set_profiling_paused(false);

    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = ctx.push("outer".into(), vec![]);

        // PHP calls pause(); span stays open.
        set_profiling_paused(true);

        // PHP returns from outer — pop runs.
        ctx.pop(id);

        let tree = ctx.finalize();
        assert_eq!(tree.finished.len(), 1);
        assert!(!tree.finished[0].leaked);
    });

    set_profiling_mode(ProfilingMode::Off);
}
