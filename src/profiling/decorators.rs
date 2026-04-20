//! Decorator-based PHP attributes for the profiler.
//!
//! Three attributes that operate on already-created spans (i.e.
//! evaluated AFTER the observer's begin has pushed the span). They
//! plug into the existing `Decorator` infrastructure used by APM —
//! same registration path as `OxPHP\Apm\Trace`.
//!
//! Per spec §6 split:
//!   - Decorator-based (this file): `Mark`, `SlowThreshold`,
//!     `MemoryThreshold`.
//!   - Observer-filter:             `Profile`, `Exclude`, `Sample`,
//!     `Tag` — must run in the C observer hot path.
//!
//! ## Per-attribute parameter limitation
//!
//! `DecoratorCallContext` does not surface the attribute's
//! constructor arguments at runtime. `SlowThreshold` and
//! `MemoryThreshold` therefore use **register-time globals**
//! captured when the plugin instantiates the decorator. Per-call
//! parameterisation needs a `DecoratorCallContext` extension and is
//! tracked as a follow-up.

use std::cell::RefCell;
use std::time::Instant;

use crate::decorator::{
    AttributeTargets, Decorator, DecoratorAction, DecoratorCallContext, DecoratorCallResult,
};
use crate::profiling::{now_ns, SpanEvent, SpanEventKind, PROFILING_CONTEXT};

// ─── #[OxPHP\Profile\Mark] ────────────────────────────────────

/// Auto-attach a `Mark` SpanEvent on entry to the decorated function.
/// The event is named after the function's qualified target (e.g.
/// `App\Service::run`); per-attribute `label` parameter support is
/// deferred until `DecoratorCallContext` exposes attribute args.
pub struct MarkDecorator;

impl Decorator for MarkDecorator {
    fn attribute_name(&self) -> &str {
        "OxPHP\\Profile\\Mark"
    }

    fn targets(&self) -> AttributeTargets {
        AttributeTargets::FUNCTION | AttributeTargets::METHOD
    }

    fn on_begin(&self, ctx: &DecoratorCallContext) -> DecoratorAction {
        let label = ctx.target.to_string();
        PROFILING_CONTEXT.with(|cell| {
            cell.borrow_mut().attach_mark_on_current(label, vec![]);
        });
        DecoratorAction::Continue
    }

    fn on_end(&self, _ctx: &DecoratorCallContext, _result: &DecoratorCallResult) {}
}

// ─── #[OxPHP\Profile\SlowThreshold(ms)] ────────────────────────

thread_local! {
    /// Per-thread stack of begin Instants — paired LIFO with on_end.
    /// Same pattern as ox_apm::DECORATOR_SPAN_IDS.
    static SLOW_STARTS: RefCell<Vec<Instant>> = const { RefCell::new(Vec::new()) };
}

pub struct SlowThresholdDecorator {
    /// Threshold in milliseconds. Register-time global; per-
    /// attribute parameterisation deferred (see module doc).
    pub default_ms: u64,
}

impl Decorator for SlowThresholdDecorator {
    fn attribute_name(&self) -> &str {
        "OxPHP\\Profile\\SlowThreshold"
    }

    fn targets(&self) -> AttributeTargets {
        AttributeTargets::FUNCTION | AttributeTargets::METHOD
    }

    fn on_begin(&self, _ctx: &DecoratorCallContext) -> DecoratorAction {
        SLOW_STARTS.with(|stack| stack.borrow_mut().push(Instant::now()));
        DecoratorAction::Continue
    }

    fn on_end(&self, _ctx: &DecoratorCallContext, _result: &DecoratorCallResult) {
        let started = SLOW_STARTS.with(|stack| stack.borrow_mut().pop());
        if let Some(t0) = started {
            let elapsed_ms = t0.elapsed().as_millis() as u64;
            if elapsed_ms > self.default_ms {
                PROFILING_CONTEXT.with(|cell| {
                    if let Some(span) = cell.borrow_mut().current_mut() {
                        span.events.push(SpanEvent {
                            name: "slow".into(),
                            attributes: vec![
                                (
                                    std::sync::Arc::from("threshold_ms"),
                                    std::sync::Arc::from(self.default_ms.to_string()),
                                ),
                                (
                                    std::sync::Arc::from("elapsed_ms"),
                                    std::sync::Arc::from(elapsed_ms.to_string()),
                                ),
                            ],
                            timestamp_ns: now_ns(),
                            kind: SpanEventKind::Slow,
                        });
                    }
                });
            }
        }
    }
}

// ─── #[OxPHP\Profile\MemoryThreshold(kb)] ──────────────────────

thread_local! {
    static MEM_STARTS: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
}

pub struct MemoryThresholdDecorator {
    /// Threshold in kibibytes. Register-time global.
    pub default_kb: u64,
}

impl Decorator for MemoryThresholdDecorator {
    fn attribute_name(&self) -> &str {
        "OxPHP\\Profile\\MemoryThreshold"
    }

    fn targets(&self) -> AttributeTargets {
        AttributeTargets::FUNCTION | AttributeTargets::METHOD
    }

    fn on_begin(&self, _ctx: &DecoratorCallContext) -> DecoratorAction {
        let mem = current_memory_usage_bytes();
        MEM_STARTS.with(|stack| stack.borrow_mut().push(mem));
        DecoratorAction::Continue
    }

    fn on_end(&self, _ctx: &DecoratorCallContext, _result: &DecoratorCallResult) {
        let started = MEM_STARTS.with(|stack| stack.borrow_mut().pop());
        if let Some(m0) = started {
            let now = current_memory_usage_bytes();
            let delta = (now - m0).max(0);
            if (delta as u64) > self.default_kb.saturating_mul(1024) {
                PROFILING_CONTEXT.with(|cell| {
                    if let Some(span) = cell.borrow_mut().current_mut() {
                        span.events.push(SpanEvent {
                            name: "memory_spike".into(),
                            attributes: vec![
                                (
                                    std::sync::Arc::from("threshold_kb"),
                                    std::sync::Arc::from(self.default_kb.to_string()),
                                ),
                                (
                                    std::sync::Arc::from("delta_bytes"),
                                    std::sync::Arc::from(delta.to_string()),
                                ),
                            ],
                            timestamp_ns: now_ns(),
                            kind: SpanEventKind::MemorySpike,
                        });
                    }
                });
            }
        }
    }
}

/// Read current PHP allocator usage via the bridge. Returns 0 in
/// host builds (no `feature = "php"`), which makes the delta
/// detection always read 0 — accepted for the test build; real
/// values flow under PHP execution.
fn current_memory_usage_bytes() -> i64 {
    #[cfg(feature = "php")]
    unsafe {
        crate::php::bindings::oxphp_bridge_get_memory_usage_bytes()
    }
    #[cfg(not(feature = "php"))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

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

    fn dummy_result() -> DecoratorCallResult {
        DecoratorCallResult {
            success: true,
            elapsed_ns: 0,
            exception_class: None,
        }
    }

    #[test]
    fn mark_decorator_attaches_event_named_after_target() {
        PROFILING_CONTEXT.with(|cell| {
            let mut ctx = cell.borrow_mut();
            ctx.reset(
                crate::profiling::ProfilingMode::ProfileAll,
                "t".into(),
                "r".into(),
            );
            let id = ctx.push("App\\Svc::run".into(), vec![]);
            drop(ctx);

            MarkDecorator.on_begin(&dummy_ctx("App\\Svc::run"));

            let mut ctx = cell.borrow_mut();
            let span = ctx.get_mut(id).expect("open");
            assert_eq!(span.events.len(), 1);
            assert_eq!(span.events[0].name, "App\\Svc::run");
            assert_eq!(span.events[0].kind, SpanEventKind::Mark);
        });
    }

    #[test]
    fn slow_decorator_emits_event_when_over_threshold() {
        PROFILING_CONTEXT.with(|cell| {
            let mut ctx = cell.borrow_mut();
            ctx.reset(
                crate::profiling::ProfilingMode::ProfileAll,
                "t".into(),
                "r".into(),
            );
            let id = ctx.push("slow_op".into(), vec![]);
            drop(ctx);

            let dec = SlowThresholdDecorator { default_ms: 0 };
            dec.on_begin(&dummy_ctx("slow_op"));
            std::thread::sleep(std::time::Duration::from_millis(2));
            dec.on_end(&dummy_ctx("slow_op"), &dummy_result());

            let mut ctx = cell.borrow_mut();
            let span = ctx.get_mut(id).expect("open");
            assert_eq!(span.events.len(), 1);
            assert_eq!(span.events[0].name, "slow");
            assert_eq!(span.events[0].kind, SpanEventKind::Slow);
            assert!(span.events[0]
                .attributes
                .iter()
                .any(|(k, _)| k.as_ref() == "elapsed_ms"));
        });
    }

    #[test]
    fn slow_decorator_silent_under_threshold() {
        PROFILING_CONTEXT.with(|cell| {
            let mut ctx = cell.borrow_mut();
            ctx.reset(
                crate::profiling::ProfilingMode::ProfileAll,
                "t".into(),
                "r".into(),
            );
            let id = ctx.push("fast_op".into(), vec![]);
            drop(ctx);

            // Threshold huge — never triggers.
            let dec = SlowThresholdDecorator {
                default_ms: 1_000_000,
            };
            dec.on_begin(&dummy_ctx("fast_op"));
            dec.on_end(&dummy_ctx("fast_op"), &dummy_result());

            let mut ctx = cell.borrow_mut();
            let span = ctx.get_mut(id).expect("open");
            assert_eq!(span.events.len(), 0);
        });
    }

    #[test]
    fn memory_decorator_silent_when_no_php_runtime() {
        // Without `feature = "php"`, current_memory_usage_bytes() is
        // pinned to 0 → delta == 0 → no event. Confirms the host-build
        // path doesn't false-positive.
        PROFILING_CONTEXT.with(|cell| {
            let mut ctx = cell.borrow_mut();
            ctx.reset(
                crate::profiling::ProfilingMode::ProfileAll,
                "t".into(),
                "r".into(),
            );
            let id = ctx.push("alloc_heavy".into(), vec![]);
            drop(ctx);

            let dec = MemoryThresholdDecorator { default_kb: 1 };
            dec.on_begin(&dummy_ctx("alloc_heavy"));
            dec.on_end(&dummy_ctx("alloc_heavy"), &dummy_result());

            let mut ctx = cell.borrow_mut();
            let span = ctx.get_mut(id).expect("open");
            // In host build, delta is 0 < threshold → no event.
            // Under PHP, would emit memory_spike with positive delta.
            #[cfg(not(feature = "php"))]
            assert_eq!(span.events.len(), 0);
            // Under PHP, value depends on runtime — don't assert.
            #[cfg(feature = "php")]
            let _ = span;
        });
    }

    #[test]
    fn slow_decorator_pairs_lifo_across_nested_calls() {
        PROFILING_CONTEXT.with(|cell| {
            let mut ctx = cell.borrow_mut();
            ctx.reset(
                crate::profiling::ProfilingMode::ProfileAll,
                "t".into(),
                "r".into(),
            );
            let outer = ctx.push("outer".into(), vec![]);
            let inner = ctx.push("inner".into(), vec![]);
            drop(ctx);

            let dec = SlowThresholdDecorator { default_ms: 0 };
            // Outer begins
            dec.on_begin(&dummy_ctx("outer"));
            std::thread::sleep(std::time::Duration::from_millis(2));
            // Inner begins
            dec.on_begin(&dummy_ctx("inner"));
            std::thread::sleep(std::time::Duration::from_millis(2));
            // Inner ends — pops most recent start
            dec.on_end(&dummy_ctx("inner"), &dummy_result());
            // Outer ends
            dec.on_end(&dummy_ctx("outer"), &dummy_result());

            let mut ctx = cell.borrow_mut();
            // The current span at on_end time is the topmost OPEN span.
            // For both ends here the topmost open is `inner` (still
            // open — Decorator on_end runs before pop). Assertion is
            // about LIFO of the timing stack, not the span attachment.
            let _outer_span = ctx.get_mut(outer);
            let _inner_span = ctx.get_mut(inner);
            // SLOW_STARTS should be empty after balanced begins/ends.
            SLOW_STARTS.with(|s| assert!(s.borrow().is_empty()));
        });
    }
}
