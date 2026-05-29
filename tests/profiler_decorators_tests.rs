//! Integration tests for the three decorator-based PHP attributes
//! (Mark, SlowThreshold, MemoryThreshold).
//!
//! Pure-Rust simulation — invokes the Decorator impls with synthetic
//! `DecoratorCallContext` and asserts the events landed on the
//! current open span. End-to-end PHP coverage runs in the Docker
//! profile (tests/php/profiler/decorator_*.php).
//!
//! The Memory decorator is exercised by a smoke test that confirms
//! it does not false-positive in the host build (where
//! `current_memory_usage_bytes()` returns 0); real memory delta
//! detection only fires under PHP and is covered in Docker.

#![cfg(feature = "plugin-profiler")]

use std::sync::Arc;

use oxphp::decorator::{
    AttrArg, AttrArgs, Decorator, DecoratorAction, DecoratorCallContext, DecoratorCallResult,
};
use oxphp::profiling::decorators::{
    MarkDecorator, MemoryThresholdDecorator, SlowThresholdDecorator,
};
use oxphp::profiling::{ProfilingMode, SpanEventKind, PROFILING_CONTEXT};

fn dummy_ctx(target: &str) -> DecoratorCallContext {
    DecoratorCallContext {
        target: Arc::from(target),
        class: None,
        method: None,
        function: Some(Arc::from(target)),
        object_id: 0,
        request_id: "req".into(),
        trace_id: "trace".into(),
        timestamp_ns: 0,
    }
}

fn dummy_result_ok() -> DecoratorCallResult {
    DecoratorCallResult {
        success: true,
        elapsed_ns: 0,
        exception_class: None,
    }
}

#[test]
fn mark_decorator_attaches_to_current_open_span() {
    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = ctx.push("App\\Repo::find".into(), vec![]);
        drop(ctx);

        let action = MarkDecorator { label: None }.on_begin(&dummy_ctx("App\\Repo::find"));
        assert_eq!(action, DecoratorAction::Continue);

        let mut ctx = cell.borrow_mut();
        let span = ctx.get_mut(id).expect("open");
        assert_eq!(span.events.len(), 1);
        assert_eq!(span.events[0].kind, SpanEventKind::Mark);
        assert_eq!(span.events[0].name, "App\\Repo::find");
    });
}

#[test]
fn slow_decorator_emits_slow_event_with_elapsed_attribute() {
    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = ctx.push("expensive".into(), vec![]);
        drop(ctx);

        let dec = SlowThresholdDecorator { ms: 0 };
        dec.on_begin(&dummy_ctx("expensive"));
        std::thread::sleep(std::time::Duration::from_millis(2));
        dec.on_end(&dummy_ctx("expensive"), &dummy_result_ok());

        let mut ctx = cell.borrow_mut();
        let span = ctx.get_mut(id).expect("open");
        assert_eq!(span.events.len(), 1);
        assert_eq!(span.events[0].name, "slow");
        assert_eq!(span.events[0].kind, SpanEventKind::Slow);
        assert!(span.events[0]
            .attributes
            .iter()
            .any(|(k, _)| k.as_ref() == "threshold_ms"));
        assert!(span.events[0]
            .attributes
            .iter()
            .any(|(k, _)| k.as_ref() == "elapsed_ms"));
    });
}

#[test]
fn slow_decorator_emits_nothing_under_threshold() {
    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = ctx.push("fast".into(), vec![]);
        drop(ctx);

        let dec = SlowThresholdDecorator { ms: 1_000_000 };
        dec.on_begin(&dummy_ctx("fast"));
        dec.on_end(&dummy_ctx("fast"), &dummy_result_ok());

        let mut ctx = cell.borrow_mut();
        let span = ctx.get_mut(id).expect("open");
        assert!(span.events.is_empty());
    });
}

#[test]
fn memory_decorator_does_not_false_positive_in_host_build() {
    // Without `feature = "php"` the bridge memory getter returns 0 →
    // delta is always 0 → no event. Covers the regression
    // "memory_spike emitted spuriously when no PHP runtime".
    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = ctx.push("alloc".into(), vec![]);
        drop(ctx);

        let dec = MemoryThresholdDecorator { kb: 1 };
        dec.on_begin(&dummy_ctx("alloc"));
        // Allocate something on the Rust side — does NOT count toward
        // PHP's zend_memory_usage, so still no event.
        let _vec: Vec<u8> = vec![0u8; 4 * 1024 * 1024];
        dec.on_end(&dummy_ctx("alloc"), &dummy_result_ok());

        let mut ctx = cell.borrow_mut();
        let span = ctx.get_mut(id).expect("open");
        // In host build (no feature=php), the decorator must stay
        // silent. Under PHP, this assertion would not hold and
        // memory_spike would fire — covered separately by Docker.
        #[cfg(not(feature = "php"))]
        assert!(
            span.events.is_empty(),
            "memory decorator must not emit without PHP runtime"
        );
        #[cfg(feature = "php")]
        let _ = span;
    });
}

#[test]
fn decorator_attribute_names_match_spec() {
    // Trivial guard against accidental rename — the C-side decorator
    // observer reads these strings to pick up our attributes.
    assert_eq!(
        MarkDecorator { label: None }.attribute_name(),
        "OxPHP\\Profile\\Mark"
    );
    assert_eq!(
        SlowThresholdDecorator { ms: 1 }.attribute_name(),
        "OxPHP\\Profile\\SlowThreshold"
    );
    assert_eq!(
        MemoryThresholdDecorator { kb: 1 }.attribute_name(),
        "OxPHP\\Profile\\MemoryThreshold"
    );
}

/// Regression guard for the bug this change fixes: the per-attribute
/// `ms` argument must reach the decorator instead of the hardcoded
/// register-time default. Drives the same factory path the resolver
/// uses (`configure`) and asserts the emitted `threshold_ms` equals the
/// attribute value — and that two different attribute values produce
/// different gating.
#[test]
fn slow_threshold_honours_per_attribute_ms() {
    // Registered template carries the default (100ms in production).
    let template = SlowThresholdDecorator { ms: 100 };

    // `#[SlowThreshold(ms: 0)]` → everything is "slow"; threshold_ms == 0.
    let strict = template
        .configure(&AttrArgs::positional(vec![AttrArg::Int(0)]))
        .expect("configure yields an instance");
    PROFILING_CONTEXT.with(|cell| {
        cell.borrow_mut()
            .reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = cell.borrow_mut().push("op".into(), vec![]);
        strict.on_begin(&dummy_ctx("op"));
        std::thread::sleep(std::time::Duration::from_millis(2));
        strict.on_end(&dummy_ctx("op"), &dummy_result_ok());

        let mut ctx = cell.borrow_mut();
        let span = ctx.get_mut(id).expect("open");
        assert_eq!(span.events.len(), 1, "ms:0 must flag a 2ms call as slow");
        let threshold = span.events[0]
            .attributes
            .iter()
            .find(|(k, _)| k.as_ref() == "threshold_ms")
            .map(|(_, v)| v.as_ref());
        assert_eq!(
            threshold,
            Some("0"),
            "threshold_ms must reflect the per-attribute ms, not the default 100"
        );
    });

    // A large per-attribute threshold → the call is NOT slow. Under the
    // old bug both attributes behaved as the hardcoded default, so this
    // case and the one above could not diverge. The threshold is set far
    // above any plausible scheduling jitter (1 h) so the negative
    // assertion can't flake under CI preemption.
    const HUGE_MS: i64 = 3_600_000;
    let lenient = template
        .configure(&AttrArgs::positional(vec![AttrArg::Int(HUGE_MS)]))
        .expect("configure yields an instance");
    PROFILING_CONTEXT.with(|cell| {
        cell.borrow_mut()
            .reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = cell.borrow_mut().push("op".into(), vec![]);
        lenient.on_begin(&dummy_ctx("op"));
        std::thread::sleep(std::time::Duration::from_millis(2));
        lenient.on_end(&dummy_ctx("op"), &dummy_result_ok());

        let mut ctx = cell.borrow_mut();
        let span = ctx.get_mut(id).expect("open");
        assert!(
            span.events.is_empty(),
            "a 2ms call must not trip a 1h threshold"
        );
    });
}

/// `#[Mark(label: "...")]` names the event after the label; a bare
/// `#[Mark]` falls back to the function target.
#[test]
fn mark_honours_per_attribute_label() {
    let template = MarkDecorator { label: None };
    let labelled = template
        .configure(&AttrArgs::positional(vec![AttrArg::Str(Arc::from(
            "checkout",
        ))]))
        .expect("configure yields an instance");
    PROFILING_CONTEXT.with(|cell| {
        cell.borrow_mut()
            .reset(ProfilingMode::ProfileAll, "t".into(), "r".into());
        let id = cell.borrow_mut().push("App\\Order::pay".into(), vec![]);
        labelled.on_begin(&dummy_ctx("App\\Order::pay"));
        let mut ctx = cell.borrow_mut();
        let span = ctx.get_mut(id).expect("open");
        assert_eq!(span.events.len(), 1);
        assert_eq!(span.events[0].name, "checkout");
    });
}
