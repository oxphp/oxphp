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

use oxphp::decorator::{Decorator, DecoratorAction, DecoratorCallContext, DecoratorCallResult};
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

        let action = MarkDecorator.on_begin(&dummy_ctx("App\\Repo::find"));
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

        let dec = SlowThresholdDecorator { default_ms: 0 };
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

        let dec = SlowThresholdDecorator {
            default_ms: 1_000_000,
        };
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

        let dec = MemoryThresholdDecorator { default_kb: 1 };
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
    assert_eq!(MarkDecorator.attribute_name(), "OxPHP\\Profile\\Mark");
    assert_eq!(
        SlowThresholdDecorator { default_ms: 1 }.attribute_name(),
        "OxPHP\\Profile\\SlowThreshold"
    );
    assert_eq!(
        MemoryThresholdDecorator { default_kb: 1 }.attribute_name(),
        "OxPHP\\Profile\\MemoryThreshold"
    );
}
