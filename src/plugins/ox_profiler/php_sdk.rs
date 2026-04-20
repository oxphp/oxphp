//! PHP SDK functions for the profiler (`OxPHP\Profile\*`).
//!
//! Seven function symbols across five conceptual operations
//! (start/stop and pause/resume each pair under one bullet in
//! spec §6). Registered unconditionally — when the profiler is
//! disabled or no profile is active for the current request,
//! the functions degrade to safe no-ops returning sentinel
//! values (`is_active() === false`; mutators / writers are silent
//! no-ops because the bridge mode flag is OFF and `current_mut()`
//! returns `None`).

use crate::bridge::call::NativeCall;
use crate::plugin::types::{PhpType, PhpValue};
use crate::plugin::PluginContext;
use crate::profiling::flush::PROFILING_MODE_PROFILE_ALL_RAW;
use crate::profiling::{
    get_profiling_mode, is_profiling_paused, set_profiling_mode, set_profiling_paused,
    ProfilingMode, PROFILING_CONTEXT,
};

/// Register the seven PHP SDK function symbols. The `_enabled` flag
/// is plumbed for parity with `ox_apm::php_sdk` but currently unused
/// — per spec §6 the SDK functions have no token check, and they
/// degrade to no-ops naturally when no profile is active.
pub fn register_functions(
    ctx: &mut PluginContext,
    _enabled: bool,
) -> Result<(), crate::plugin::PluginError> {
    // OxPHP\Profile\is_active(): bool — true iff bridge mode is
    // PROFILE_ALL and not paused. Two TLS reads, no FFI hop into
    // PROFILING_CONTEXT (cheaper).
    ctx.function("OxPHP\\Profile\\is_active")
        .returns(PhpType::Bool)
        .handler(|call: &mut NativeCall| {
            let active =
                get_profiling_mode() == PROFILING_MODE_PROFILE_ALL_RAW && !is_profiling_paused();
            call.ret_bool(active);
            Ok(())
        })?;

    // OxPHP\Profile\start(): void — enable capture for the rest of
    // the request. Sets bridge mode to PROFILE_ALL, clears paused,
    // and promotes PROFILING_CONTEXT.mode if it was lower (preserving
    // trace_id / root_span_id). Caveat: a mid-request promotion via
    // reset() clears any spans already collected — this matches the
    // spec invariant that mode is set at most once per request,
    // either by the trigger at RINIT or here from PHP.
    ctx.function("OxPHP\\Profile\\start")
        .returns(PhpType::Void)
        .handler(|_call: &mut NativeCall| {
            set_profiling_mode(ProfilingMode::ProfileAll);
            set_profiling_paused(false);
            PROFILING_CONTEXT.with(|cell| {
                let mut ctx = cell.borrow_mut();
                if ctx.mode != ProfilingMode::ProfileAll {
                    let trace_id = ctx.trace_id().to_string();
                    let root = ctx.root_span_id().to_string();
                    ctx.reset(ProfilingMode::ProfileAll, trace_id, root);
                }
            });
            Ok(())
        })?;

    // OxPHP\Profile\stop(): void — disable further capture. Open
    // spans close naturally as PHP returns from them (the C end
    // callback intentionally ignores the paused flag so open_stack
    // stays balanced).
    ctx.function("OxPHP\\Profile\\stop")
        .returns(PhpType::Void)
        .handler(|_call: &mut NativeCall| {
            set_profiling_paused(true);
            Ok(())
        })?;

    // OxPHP\Profile\pause(): void — soft variant of stop. Same
    // mechanism (paused flag); the distinction is documentary intent.
    ctx.function("OxPHP\\Profile\\pause")
        .returns(PhpType::Void)
        .handler(|_call: &mut NativeCall| {
            set_profiling_paused(true);
            Ok(())
        })?;

    // OxPHP\Profile\resume(): void — clear pause.
    ctx.function("OxPHP\\Profile\\resume")
        .returns(PhpType::Void)
        .handler(|_call: &mut NativeCall| {
            set_profiling_paused(false);
            Ok(())
        })?;

    // OxPHP\Profile\mark(string $label, array $attrs = []): void —
    // attach a Mark event to the topmost open span. No-op when no
    // span is open.
    ctx.function("OxPHP\\Profile\\mark")
        .param("label", PhpType::String)
        .optional_param("attrs", PhpType::Array, PhpValue::Null)
        .returns(PhpType::Void)
        .handler(|call: &mut NativeCall| {
            let label = call.arg_str(0).unwrap_or("").to_string();
            let attrs = read_string_attrs(call, 1);
            PROFILING_CONTEXT.with(|cell| {
                cell.borrow_mut().attach_mark_on_current(label, attrs);
            });
            Ok(())
        })?;

    // OxPHP\Profile\metric(string $name, float $value): void —
    // append "metric.<name>" = "<value>" to the current span's
    // attributes. No-op when no span is open.
    ctx.function("OxPHP\\Profile\\metric")
        .param("name", PhpType::String)
        .param("value", PhpType::Float)
        .returns(PhpType::Void)
        .handler(|call: &mut NativeCall| {
            let name = call.arg_str(0).unwrap_or("").to_string();
            let value = call.arg_double(1).unwrap_or(0.0);
            PROFILING_CONTEXT.with(|cell| {
                cell.borrow_mut().attach_metric_on_current(&name, value);
            });
            Ok(())
        })?;

    Ok(())
}

/// Read an optional PHP array argument as `Vec<(String, String)>`.
/// Mirrors the pattern used by `ox_apm::php_sdk` — string-coerces
/// keys, falls back to `""` for non-string values. Missing / null
/// argument yields the empty vec.
fn read_string_attrs(
    call: &mut NativeCall,
    idx: u32,
) -> Vec<(std::sync::Arc<str>, std::sync::Arc<str>)> {
    let mut attrs: Vec<(std::sync::Arc<str>, std::sync::Arc<str>)> = Vec::new();
    if call.argc() <= idx {
        return attrs;
    }
    if !matches!(call.arg_is_null(idx), Ok(false)) {
        return attrs;
    }
    let _ = call.arg_array_foreach(idx, |k, v| {
        let key: std::sync::Arc<str> = match k {
            crate::bridge::call::ArrayKey::Str(s) => std::sync::Arc::from(s),
            crate::bridge::call::ArrayKey::Int(i) => std::sync::Arc::from(i.to_string()),
        };
        let val: std::sync::Arc<str> = std::sync::Arc::from(v.as_str().unwrap_or(""));
        attrs.push((key, val));
    });
    attrs
}
