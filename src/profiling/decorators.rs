//! Decorator-based PHP attributes for the profiler.
//!
//! Three attributes that operate on already-created spans (i.e.
//! evaluated AFTER the observer's begin has pushed the span). They
//! plug into the existing `Decorator` infrastructure used by APM —
//! same registration path as `OxPHP\Apm\Trace`.
//!
//! Split:
//!   - Decorator-based (this file): `Mark`, `SlowThreshold`,
//!     `MemoryThreshold`.
//!   - Observer-filter:             `Profile`, `Exclude`, `Sample`,
//!     `Tag` — must run in the C observer hot path.
//!
//! ## Per-attribute parameters
//!
//! Each decorator is a factory: the registered instance carries the
//! global default, and [`Decorator::configure`] builds a per-attribute
//! instance from the attribute's constructor arguments at resolve time.
//! `#[SlowThreshold(ms: 250)]` therefore produces an instance with
//! `ms = 250`; a bare `#[SlowThreshold]` falls back to the registered
//! default. `on_begin`/`on_end` read the configured `self` — no
//! per-call argument lookup on the hot path.

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Instant;

use crate::decorator::{
    AttrArgs, AttributeTargets, Decorator, DecoratorAction, DecoratorCallContext,
    DecoratorCallResult,
};
use crate::profiling::{now_ns, SpanEvent, SpanEventKind, PROFILING_CONTEXT};

// ─── #[OxPHP\Profile\Mark] ────────────────────────────────────

/// Auto-attach a `Mark` SpanEvent on entry to the decorated function.
/// The event is named after the attribute's `label` argument
/// (`#[Mark(label: "checkout")]`), falling back to the function's
/// qualified target (e.g. `App\Service::run`) for a bare `#[Mark]`.
pub struct MarkDecorator {
    /// Explicit label from `#[Mark(label: ...)]`. `None` on the
    /// registered instance and for a bare `#[Mark]` — falls back to the
    /// function target.
    pub label: Option<Arc<str>>,
}

impl Decorator for MarkDecorator {
    fn attribute_name(&self) -> &str {
        "OxPHP\\Profile\\Mark"
    }

    fn targets(&self) -> AttributeTargets {
        AttributeTargets::FUNCTION | AttributeTargets::METHOD
    }

    fn on_begin(&self, ctx: &DecoratorCallContext) -> DecoratorAction {
        let label = match &self.label {
            Some(l) => l.to_string(),
            None => ctx.target.to_string(),
        };
        PROFILING_CONTEXT.with(|cell| {
            cell.borrow_mut().attach_mark_on_current(label, vec![]);
        });
        DecoratorAction::Continue
    }

    fn on_end(&self, _ctx: &DecoratorCallContext, _result: &DecoratorCallResult) {}

    fn configure(&self, args: &AttrArgs) -> Option<Arc<dyn Decorator>> {
        // `#[Mark(label: "x")]` (named) or `#[Mark("x")]` (positional).
        let label = args
            .str_named("label")
            .or_else(|| args.str(0))
            .map(Arc::from);
        Some(Arc::new(Self { label }))
    }
}

// ─── #[OxPHP\Profile\SlowThreshold(ms)] ────────────────────────

thread_local! {
    /// Per-thread stack of begin Instants — paired LIFO with on_end.
    /// Same pattern as ox_apm::DECORATOR_SPAN_IDS.
    static SLOW_STARTS: RefCell<Vec<Instant>> = const { RefCell::new(Vec::new()) };
}

pub struct SlowThresholdDecorator {
    /// Threshold in milliseconds. The registered instance holds the
    /// global default; `configure` overrides it per attribute from
    /// `#[SlowThreshold(ms: N)]`.
    pub ms: u64,
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
            if elapsed_ms > self.ms {
                PROFILING_CONTEXT.with(|cell| {
                    if let Some(span) = cell.borrow_mut().current_mut() {
                        span.events.push(SpanEvent {
                            name: "slow".into(),
                            attributes: vec![
                                (Arc::from("threshold_ms"), Arc::from(self.ms.to_string())),
                                (Arc::from("elapsed_ms"), Arc::from(elapsed_ms.to_string())),
                            ],
                            timestamp_ns: now_ns(),
                            kind: SpanEventKind::Slow,
                        });
                    }
                });
            }
        }
    }

    fn configure(&self, args: &AttrArgs) -> Option<Arc<dyn Decorator>> {
        // `#[SlowThreshold(ms: N)]` (named) or `#[SlowThreshold(N)]`.
        let ms = args
            .int_named("ms")
            .or_else(|| args.int(0))
            .map(|v| v.max(0) as u64)
            .unwrap_or(self.ms);
        Some(Arc::new(Self { ms }))
    }
}

// ─── #[OxPHP\Profile\MemoryThreshold(kb)] ──────────────────────

thread_local! {
    static MEM_STARTS: RefCell<Vec<i64>> = const { RefCell::new(Vec::new()) };
}

pub struct MemoryThresholdDecorator {
    /// Threshold in kibibytes. The registered instance holds the global
    /// default; `configure` overrides it per attribute from
    /// `#[MemoryThreshold(kb: N)]`.
    pub kb: u64,
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
            if (delta as u64) > self.kb.saturating_mul(1024) {
                PROFILING_CONTEXT.with(|cell| {
                    if let Some(span) = cell.borrow_mut().current_mut() {
                        span.events.push(SpanEvent {
                            name: "memory_spike".into(),
                            attributes: vec![
                                (Arc::from("threshold_kb"), Arc::from(self.kb.to_string())),
                                (Arc::from("delta_bytes"), Arc::from(delta.to_string())),
                            ],
                            timestamp_ns: now_ns(),
                            kind: SpanEventKind::MemorySpike,
                        });
                    }
                });
            }
        }
    }

    fn configure(&self, args: &AttrArgs) -> Option<Arc<dyn Decorator>> {
        // `#[MemoryThreshold(kb: N)]` (named) or `#[MemoryThreshold(N)]`.
        let kb = args
            .int_named("kb")
            .or_else(|| args.int(0))
            .map(|v| v.max(0) as u64)
            .unwrap_or(self.kb);
        Some(Arc::new(Self { kb }))
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
    use crate::decorator::AttrArg;

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

            MarkDecorator { label: None }.on_begin(&dummy_ctx("App\\Svc::run"));

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

            let dec = SlowThresholdDecorator { ms: 0 };
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
            let dec = SlowThresholdDecorator { ms: 1_000_000 };
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

            let dec = MemoryThresholdDecorator { kb: 1 };
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

            let dec = SlowThresholdDecorator { ms: 0 };
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

    // ── configure(): per-attribute parameterisation ──

    /// A configured SlowThreshold uses the per-attribute `ms`, not the
    /// registered default. This is the regression guard for the bug
    /// where `#[SlowThreshold(ms: N)]` silently ignored `N`.
    #[test]
    fn slow_configure_uses_per_attribute_ms() {
        let template = SlowThresholdDecorator { ms: 100 };
        // Threshold far above any scheduling jitter so the negative
        // assertion below can't flake under CI preemption.
        let configured = template
            .configure(&AttrArgs::positional(vec![AttrArg::Int(3_600_000)]))
            .expect("configure yields an instance");

        PROFILING_CONTEXT.with(|cell| {
            cell.borrow_mut().reset(
                crate::profiling::ProfilingMode::ProfileAll,
                "t".into(),
                "r".into(),
            );
            let id = cell.borrow_mut().push("op".into(), vec![]);

            configured.on_begin(&dummy_ctx("op"));
            std::thread::sleep(std::time::Duration::from_millis(2));
            configured.on_end(&dummy_ctx("op"), &dummy_result());

            let mut ctx = cell.borrow_mut();
            let span = ctx.get_mut(id).expect("open");
            // A 2ms call is under the configured threshold → no slow
            // event. (The configured value gates, not the template's 100.)
            assert!(
                span.events.is_empty(),
                "2ms must be under the configured threshold"
            );
        });

        // And the emitted threshold_ms reflects the per-attribute value.
        PROFILING_CONTEXT.with(|cell| {
            cell.borrow_mut().reset(
                crate::profiling::ProfilingMode::ProfileAll,
                "t".into(),
                "r".into(),
            );
            let id = cell.borrow_mut().push("op".into(), vec![]);
            let fast = template
                .configure(&AttrArgs::positional(vec![AttrArg::Int(0)]))
                .expect("configure yields an instance");
            fast.on_begin(&dummy_ctx("op"));
            std::thread::sleep(std::time::Duration::from_millis(2));
            fast.on_end(&dummy_ctx("op"), &dummy_result());

            let mut ctx = cell.borrow_mut();
            let span = ctx.get_mut(id).expect("open");
            assert_eq!(span.events.len(), 1);
            let threshold = span.events[0]
                .attributes
                .iter()
                .find(|(k, _)| k.as_ref() == "threshold_ms")
                .map(|(_, v)| v.as_ref());
            assert_eq!(threshold, Some("0"), "threshold_ms reflects configured ms");
        });
    }

    /// A bare `#[SlowThreshold]` (no args) falls back to the registered
    /// default.
    #[test]
    fn slow_configure_falls_back_to_default_without_args() {
        let template = SlowThresholdDecorator { ms: 1_000_000 };
        let configured = template
            .configure(&AttrArgs::default())
            .expect("configure yields an instance");

        PROFILING_CONTEXT.with(|cell| {
            cell.borrow_mut().reset(
                crate::profiling::ProfilingMode::ProfileAll,
                "t".into(),
                "r".into(),
            );
            let id = cell.borrow_mut().push("op".into(), vec![]);
            configured.on_begin(&dummy_ctx("op"));
            std::thread::sleep(std::time::Duration::from_millis(2));
            configured.on_end(&dummy_ctx("op"), &dummy_result());

            let mut ctx = cell.borrow_mut();
            let span = ctx.get_mut(id).expect("open");
            // 2ms well under the 1_000_000ms default → silent.
            assert!(span.events.is_empty());
        });
    }

    /// MemoryThreshold's per-attribute `kb` only changes observable
    /// behaviour under a real PHP allocator (host build reads 0 bytes),
    /// so the spike path is covered in the Docker profile. Here we just
    /// confirm `configure` always yields an instance, with and without
    /// an argument.
    #[test]
    fn memory_configure_yields_instance() {
        let template = MemoryThresholdDecorator { kb: 64 };
        assert!(template
            .configure(&AttrArgs::positional(vec![AttrArg::Int(2048)]))
            .is_some());
        assert!(template.configure(&AttrArgs::default()).is_some());
    }

    #[test]
    fn mark_configure_uses_label_then_falls_back_to_target() {
        // With a label argument the Mark event is named after the label.
        let labelled = MarkDecorator { label: None }
            .configure(&AttrArgs::positional(vec![AttrArg::Str(Arc::from(
                "checkout",
            ))]))
            .expect("configure yields an instance");
        PROFILING_CONTEXT.with(|cell| {
            cell.borrow_mut().reset(
                crate::profiling::ProfilingMode::ProfileAll,
                "t".into(),
                "r".into(),
            );
            let id = cell.borrow_mut().push("App\\Order::pay".into(), vec![]);
            labelled.on_begin(&dummy_ctx("App\\Order::pay"));
            let mut ctx = cell.borrow_mut();
            let span = ctx.get_mut(id).expect("open");
            assert_eq!(span.events.len(), 1);
            assert_eq!(span.events[0].name, "checkout");
        });

        // A bare #[Mark] (no label arg) falls back to the function target.
        let bare = MarkDecorator { label: None }
            .configure(&AttrArgs::default())
            .expect("configure yields an instance");
        PROFILING_CONTEXT.with(|cell| {
            cell.borrow_mut().reset(
                crate::profiling::ProfilingMode::ProfileAll,
                "t".into(),
                "r".into(),
            );
            let id = cell.borrow_mut().push("App\\Order::pay".into(), vec![]);
            bare.on_begin(&dummy_ctx("App\\Order::pay"));
            let mut ctx = cell.borrow_mut();
            let span = ctx.get_mut(id).expect("open");
            assert_eq!(span.events.len(), 1);
            assert_eq!(span.events[0].name, "App\\Order::pay");
        });
    }

    /// The built-ins read their argument by parameter name, so a
    /// `name:`-style attribute (`#[Mark(label: "x")]`,
    /// `#[SlowThreshold(ms: N)]`) configures correctly even though the
    /// name is the only thing distinguishing it.
    #[test]
    fn builtins_read_named_arguments() {
        let labelled = MarkDecorator { label: None }
            .configure(&AttrArgs::from_pairs(vec![(
                Some(Arc::from("label")),
                AttrArg::Str(Arc::from("checkout")),
            )]))
            .expect("configure yields an instance");
        PROFILING_CONTEXT.with(|cell| {
            cell.borrow_mut().reset(
                crate::profiling::ProfilingMode::ProfileAll,
                "t".into(),
                "r".into(),
            );
            let id = cell.borrow_mut().push("App\\Order::pay".into(), vec![]);
            labelled.on_begin(&dummy_ctx("App\\Order::pay"));
            let mut ctx = cell.borrow_mut();
            let span = ctx.get_mut(id).expect("open");
            assert_eq!(span.events[0].name, "checkout");
        });

        let fast = SlowThresholdDecorator { ms: 1_000_000 }
            .configure(&AttrArgs::from_pairs(vec![(
                Some(Arc::from("ms")),
                AttrArg::Int(0),
            )]))
            .expect("configure yields an instance");
        PROFILING_CONTEXT.with(|cell| {
            cell.borrow_mut().reset(
                crate::profiling::ProfilingMode::ProfileAll,
                "t".into(),
                "r".into(),
            );
            let id = cell.borrow_mut().push("op".into(), vec![]);
            fast.on_begin(&dummy_ctx("op"));
            std::thread::sleep(std::time::Duration::from_millis(2));
            fast.on_end(&dummy_ctx("op"), &dummy_result());
            let mut ctx = cell.borrow_mut();
            let span = ctx.get_mut(id).expect("open");
            // Named ms:0 gates, not the 1_000_000 default → slow event.
            assert_eq!(span.events.len(), 1);
        });
    }
}
