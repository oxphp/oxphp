//! PHP SDK functions for APM tracing (`oxphp_apm_*`).
//!
//! All 10 functions are registered regardless of whether APM is enabled.
//! When disabled, the span functions are no-ops returning sentinel values.
//! `oxphp_apm_trace` is the exception on both counts: it runs its callback
//! either way — dropping it would drop business logic, not just a span — so
//! it also propagates whatever that callback throws, and rejects a second
//! argument that is not callable, with APM on or off.

use crate::bridge::call::{CallableOutcome, NativeCall};
use crate::plugin::types::{PhpType, PhpValue};
use crate::plugin::PluginContext;

use crate::profiling::{now_ns, SpanEvent, SpanEventKind, PROFILING_CONTEXT};

/// Register all `oxphp_apm_*` PHP functions.
///
/// The `enabled` flag controls runtime behavior: when `false`, functions
/// return sentinel values (0 for IDs, "" for strings) without touching
/// the span stack — except `oxphp_apm_trace`, which returns whatever its
/// callback returned and is handed span id 0 in place of a real one.
pub fn register_functions(
    ctx: &mut PluginContext,
    enabled: bool,
) -> Result<(), crate::plugin::PluginError> {
    // 1. oxphp_apm_trace(name, callback, ?attributes)
    ctx.function("oxphp_apm_trace")
        .param("name", PhpType::String)
        .param("callback", PhpType::Mixed)
        .optional_param("attributes", PhpType::Array, PhpValue::Null)
        .returns(PhpType::Mixed)
        .handler(move |call: &mut NativeCall| {
            // With APM off the span is what disappears, not the business logic:
            // the callback still runs and still gets its value forwarded, it is
            // just handed span id 0 and nothing is recorded.
            if !enabled {
                let outcome = call.call_arg_callable(1, 1, |b| b.long(0));
                return finish_trace_callback(call, outcome);
            }

            let name = match call.arg_str(0) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    let outcome = call.call_arg_callable(1, 1, |b| b.long(0));
                    return finish_trace_callback(call, outcome);
                }
            };
            let attrs = collect_attributes(call, 2);

            // Push, then drop the borrow before the callback runs: the callback
            // is free to call oxphp_apm_attribute() / _event() / _span_id(),
            // all of which borrow PROFILING_CONTEXT again.
            let span_id = PROFILING_CONTEXT
                .with(|stack| stack.borrow_mut().push(std::sync::Arc::from(name), attrs));

            let outcome = call.call_arg_callable(1, 1, |b| b.long(span_id as i64));

            // Read the exception before taking the borrow, as oxphp_apm_error()
            // does — so borrow-safety here does not rest on what reading a
            // Throwable's class and message happens to touch.
            let captured = match &outcome {
                Ok(CallableOutcome::Threw) => Some(capture_pending_exception()),
                _ => None,
            };

            // Close the span on every path — returned, threw, or refused.
            PROFILING_CONTEXT.with(|stack| {
                let mut stack = stack.borrow_mut();
                if let Some((class, message)) = &captured {
                    if let Some(span) = stack.get_mut(span_id) {
                        span.status_code = 2; // Error
                        super::push_exception_event(
                            span,
                            class,
                            Some(message.as_str()),
                            // No stacktrace: unlike the decorator path, this one
                            // runs on every exception that escapes a traced
                            // call, and getTraceAsString() re-enters the VM.
                            None,
                            super::MESSAGE_MAX_BYTES.load(std::sync::atomic::Ordering::Relaxed),
                            super::STACKTRACE_MAX_BYTES.load(std::sync::atomic::Ordering::Relaxed),
                        );
                    }
                }
                stack.pop(span_id);
            });

            finish_trace_callback(call, outcome)
        })?;

    // 2. oxphp_apm_start(name, ?attributes)
    ctx.function("oxphp_apm_start")
        .param("name", PhpType::String)
        .optional_param("attributes", PhpType::Array, PhpValue::Null)
        .returns(PhpType::Int)
        .handler(move |call: &mut NativeCall| {
            if !enabled {
                call.ret_long(0);
                return Ok(());
            }

            let name = match call.arg_str(0) {
                Ok(s) => s.to_string(),
                Err(_) => {
                    call.ret_long(0);
                    return Ok(());
                }
            };

            let attrs = collect_attributes(call, 1);

            let local_id = PROFILING_CONTEXT
                .with(|stack| stack.borrow_mut().push(std::sync::Arc::from(name), attrs));
            call.ret_long(local_id as i64);
            Ok(())
        })?;

    // 3. oxphp_apm_end(span_id)
    ctx.function("oxphp_apm_end")
        .param("span_id", PhpType::Int)
        .returns(PhpType::Void)
        .handler(move |call: &mut NativeCall| {
            if !enabled {
                return Ok(());
            }

            let span_id = match call.arg_long(0) {
                Ok(id) => id as u32,
                Err(_) => return Ok(()),
            };

            PROFILING_CONTEXT.with(|stack| {
                stack.borrow_mut().pop(span_id);
            });
            Ok(())
        })?;

    // 4. oxphp_apm_attribute(key, value, ?span_id)
    ctx.function("oxphp_apm_attribute")
        .param("key", PhpType::String)
        .param("value", PhpType::Mixed)
        .optional_param("span_id", PhpType::Int, PhpValue::Null)
        .returns(PhpType::Void)
        .handler(move |call: &mut NativeCall| {
            if !enabled {
                return Ok(());
            }

            let key: std::sync::Arc<str> = match call.arg_str(0) {
                Ok(s) => std::sync::Arc::from(s),
                Err(_) => return Ok(()),
            };

            // Read value as string for now (mixed type conversion is complex)
            let value: std::sync::Arc<str> = std::sync::Arc::from(read_mixed_as_string(call, 1));

            // Determine target span: explicit span_id or current
            let explicit_id = if call.argc() > 2 {
                match call.arg_is_null(2) {
                    Ok(true) => None,
                    Ok(false) => call
                        .arg_long(2)
                        .ok()
                        .map(|id| id as u32)
                        .filter(|&id| id != 0),
                    Err(_) => None,
                }
            } else {
                None
            };

            PROFILING_CONTEXT.with(|stack| {
                let mut stack = stack.borrow_mut();
                let span = if let Some(id) = explicit_id {
                    stack.get_mut(id)
                } else {
                    stack.current_mut()
                };
                if let Some(span) = span {
                    span.attributes.push((key, value));
                }
            });
            Ok(())
        })?;

    // 5. oxphp_apm_event(name, ?attributes, ?span_id)
    ctx.function("oxphp_apm_event")
        .param("name", PhpType::String)
        .optional_param("attributes", PhpType::Array, PhpValue::Null)
        .optional_param("span_id", PhpType::Int, PhpValue::Null)
        .returns(PhpType::Void)
        .handler(move |call: &mut NativeCall| {
            if !enabled {
                return Ok(());
            }

            let name = match call.arg_str(0) {
                Ok(s) => s.to_string(),
                Err(_) => return Ok(()),
            };

            let attrs = collect_attributes(call, 1);

            let explicit_id = if call.argc() > 2 {
                match call.arg_is_null(2) {
                    Ok(true) => None,
                    Ok(false) => call
                        .arg_long(2)
                        .ok()
                        .map(|id| id as u32)
                        .filter(|&id| id != 0),
                    Err(_) => None,
                }
            } else {
                None
            };

            let event = SpanEvent {
                name,
                attributes: attrs,
                timestamp_ns: now_ns(),
                kind: SpanEventKind::Custom,
            };

            PROFILING_CONTEXT.with(|stack| {
                let mut stack = stack.borrow_mut();
                let span = if let Some(id) = explicit_id {
                    stack.get_mut(id)
                } else {
                    stack.current_mut()
                };
                if let Some(span) = span {
                    span.events.push(event);
                }
            });
            Ok(())
        })?;

    // 6. oxphp_apm_error(exception, ?span_id)
    ctx.function("oxphp_apm_error")
        .param("exception", PhpType::Mixed)
        .optional_param("span_id", PhpType::Int, PhpValue::Null)
        .returns(PhpType::Void)
        .handler(move |call: &mut NativeCall| {
            if !enabled {
                return Ok(());
            }

            let explicit_id = if call.argc() > 1 {
                match call.arg_is_null(1) {
                    Ok(true) => None,
                    Ok(false) => call
                        .arg_long(1)
                        .ok()
                        .map(|id| id as u32)
                        .filter(|&id| id != 0),
                    Err(_) => None,
                }
            } else {
                None
            };

            // Capture the exception data BEFORE touching the span stack. The
            // capture re-enters the VM (getTraceAsString()); doing it with no
            // PROFILING_CONTEXT borrow held mirrors the decorator path, which
            // finishes capture in C before any Rust borrow. getTraceAsString is
            // final so the re-entry cannot call back into oxphp_apm_* today, but
            // capturing first keeps borrow-safety from resting on that invariant.
            //
            // A Throwable yields type/message/stacktrace; a bare string reason
            // (oxphp_apm_error('gateway timeout')) is recorded as the message
            // under a generic "Error" type so the event still renders in backends
            // that key exception events on exception.type (e.g. New Relic Errors
            // Inbox) rather than being silently dropped.
            let mut captured: Option<(String, Option<String>, Option<String>)> = None;
            if call.argc() > 0 {
                call.capture_exception_arg(0, |cls, msg, trace| {
                    captured = Some((
                        cls.to_string(),
                        msg.map(String::from),
                        trace.map(String::from),
                    ));
                });
                if captured.is_none() {
                    if let Ok(crate::bridge::types::ValType::String) = call.arg_type(0) {
                        if let Ok(bytes) = call.arg_bytes(0) {
                            if !bytes.is_empty() {
                                captured = Some((
                                    "Error".to_string(),
                                    Some(String::from_utf8_lossy(bytes).into_owned()),
                                    None,
                                ));
                            }
                        }
                    }
                }
            }

            // Resolve the target span once, mark it errored, and record the
            // captured exception (if any) on that same span.
            PROFILING_CONTEXT.with(|stack| {
                let mut stack = stack.borrow_mut();
                let span = if let Some(id) = explicit_id {
                    stack.get_mut(id)
                } else {
                    stack.current_mut()
                };
                let span = match span {
                    Some(span) => span,
                    None => return,
                };
                span.status_code = 2; // Error
                if let Some((cls, msg, trace)) = &captured {
                    super::push_exception_event(
                        span,
                        cls,
                        msg.as_deref(),
                        trace.as_deref(),
                        super::MESSAGE_MAX_BYTES.load(std::sync::atomic::Ordering::Relaxed),
                        super::STACKTRACE_MAX_BYTES.load(std::sync::atomic::Ordering::Relaxed),
                    );
                }
            });
            Ok(())
        })?;

    // 7. oxphp_apm_status(code, ?description, ?span_id)
    ctx.function("oxphp_apm_status")
        .param("code", PhpType::Int)
        .optional_param("description", PhpType::String, PhpValue::Null)
        .optional_param("span_id", PhpType::Int, PhpValue::Null)
        .returns(PhpType::Void)
        .handler(move |call: &mut NativeCall| {
            if !enabled {
                return Ok(());
            }

            let code = match call.arg_long(0) {
                Ok(c) => c as u8,
                Err(_) => return Ok(()),
            };

            let description = if call.argc() > 1 {
                match call.arg_is_null(1) {
                    Ok(true) => None,
                    Ok(false) => call.arg_str(1).ok().map(|s| s.to_string()),
                    Err(_) => None,
                }
            } else {
                None
            };

            let explicit_id = if call.argc() > 2 {
                match call.arg_is_null(2) {
                    Ok(true) => None,
                    Ok(false) => call
                        .arg_long(2)
                        .ok()
                        .map(|id| id as u32)
                        .filter(|&id| id != 0),
                    Err(_) => None,
                }
            } else {
                None
            };

            PROFILING_CONTEXT.with(|stack| {
                let mut stack = stack.borrow_mut();
                let span = if let Some(id) = explicit_id {
                    stack.get_mut(id)
                } else {
                    stack.current_mut()
                };
                if let Some(span) = span {
                    span.status_code = code;
                    span.status_message = description;
                }
            });
            Ok(())
        })?;

    // 8. oxphp_apm_trace_id() — no params
    ctx.function("oxphp_apm_trace_id")
        .returns(PhpType::String)
        .handler(move |call: &mut NativeCall| {
            if !enabled {
                call.ret_str("");
                return Ok(());
            }

            PROFILING_CONTEXT.with(|stack| {
                let stack = stack.borrow();
                let tid = stack.trace_id();
                if tid.is_empty() {
                    call.ret_str("");
                } else {
                    call.ret_str(tid);
                }
            });
            Ok(())
        })?;

    // 9. oxphp_apm_span_id() — no params
    ctx.function("oxphp_apm_span_id")
        .returns(PhpType::String)
        .handler(move |call: &mut NativeCall| {
            if !enabled {
                call.ret_str("");
                return Ok(());
            }

            PROFILING_CONTEXT.with(|stack| {
                let stack = stack.borrow();
                let span_id = stack.current().map(|s| s.span_id.as_ref()).unwrap_or("");
                call.ret_str(span_id);
            });
            Ok(())
        })?;

    // 10. oxphp_apm_header() — no params
    ctx.function("oxphp_apm_header")
        .returns(PhpType::String)
        .handler(move |call: &mut NativeCall| {
            if !enabled {
                call.ret_str("");
                return Ok(());
            }

            PROFILING_CONTEXT.with(|stack| {
                let stack = stack.borrow();
                let trace_id = stack.trace_id();
                if trace_id.is_empty() {
                    call.ret_str("");
                    return;
                }
                // Prefer the current open SDK span. With none open (a call
                // at the top level of the request), fall back to the request's
                // root span so downstream work is parented to this request
                // rather than starting an orphan trace.
                let span_id = match stack.current() {
                    Some(s) => s.span_id.as_ref(),
                    None => stack.root_span_id(),
                };
                if span_id.is_empty() {
                    call.ret_str("");
                    return;
                }
                let header = format!("00-{trace_id}-{span_id}-01");
                call.ret_str(&header);
            });
            Ok(())
        })?;

    Ok(())
}

/// Turn the result of invoking the `oxphp_apm_trace()` callback into the
/// function's own outcome: forward the return value, let a pending exception
/// propagate untouched, or reject a second argument that was never callable.
///
/// The pending-exception path deliberately returns `Ok(())`. Returning `Err`
/// would send the dispatcher through `oxphp_throw_exception()`, replacing the
/// exception already in flight — the callback's, or one raised while resolving
/// the callback — with one of ours.
fn finish_trace_callback(
    call: &mut NativeCall,
    outcome: Result<CallableOutcome, crate::plugin::php::PhpError>,
) -> Result<(), crate::plugin::php::PhpError> {
    match outcome? {
        CallableOutcome::Returned(value) => {
            // retval is the IS_NULL slot Zend initialized for this internal
            // function and nothing here has written it, so overwriting the
            // bytes releases nothing.
            value.write_into(call.retval_ptr());
            Ok(())
        }
        CallableOutcome::Threw => Ok(()),
        CallableOutcome::NotCallable => Err(crate::plugin::php::PhpError::Exception {
            class: "TypeError".to_string(),
            message: "oxphp_apm_trace(): Argument #2 ($callback) must be a valid callback"
                .to_string(),
            code: 0,
        }),
    }
}

/// Class and message of the pending PHP exception, empty strings if none.
fn capture_pending_exception() -> (String, String) {
    use crate::bridge::ffi as bridge_ffi;
    use std::ffi::CStr;

    let mut class_p: *const std::os::raw::c_char = std::ptr::null();
    let mut msg_p: *const std::os::raw::c_char = std::ptr::null();
    let mut code: i64 = 0;
    unsafe {
        if bridge_ffi::oxphp_exception_pending() != 0 {
            bridge_ffi::oxphp_exception_get(&mut class_p, &mut msg_p, &mut code);
        }
    }
    let to_string = |p: *const std::os::raw::c_char| -> String {
        if p.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    };
    (to_string(class_p), to_string(msg_p))
}

/// Collect the optional attribute array at argument `idx` into span attributes.
///
/// Shared by `oxphp_apm_trace`, `oxphp_apm_start` and `oxphp_apm_event`, which
/// all take the same optional array in the same shape.
fn collect_attributes(
    call: &NativeCall,
    idx: u32,
) -> Vec<(std::sync::Arc<str>, std::sync::Arc<str>)> {
    let mut attrs: Vec<(std::sync::Arc<str>, std::sync::Arc<str>)> = Vec::new();
    if call.argc() > idx {
        if let Ok(false) = call.arg_is_null(idx) {
            let _ = call.arg_array_foreach(idx, |k, v| {
                let key: std::sync::Arc<str> = match k {
                    crate::bridge::call::ArrayKey::Str(s) => std::sync::Arc::from(s),
                    crate::bridge::call::ArrayKey::Int(i) => {
                        std::sync::Arc::from(i.to_string().as_str())
                    }
                };
                let val: std::sync::Arc<str> = std::sync::Arc::from(v.as_str().unwrap_or(""));
                attrs.push((key, val));
            });
        }
    }
    attrs
}

/// Read a mixed-type argument as a string representation.
///
/// For non-critical attribute values, we convert whatever PHP passes
/// into a string rather than requiring a specific type.
fn read_mixed_as_string(call: &NativeCall, idx: u32) -> String {
    use crate::bridge::types::ValType;

    let t = match call.arg_type(idx) {
        Ok(t) => t,
        Err(_) => return String::new(),
    };
    match t {
        ValType::String => call.arg_str(idx).unwrap_or("").to_string(),
        ValType::Long => call
            .arg_long(idx)
            .map(|v| v.to_string())
            .unwrap_or_default(),
        ValType::Double => call
            .arg_double(idx)
            .map(|v| v.to_string())
            .unwrap_or_default(),
        ValType::True => "true".to_string(),
        ValType::False => "false".to_string(),
        ValType::Null => "null".to_string(),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventDispatcher;
    use crate::plugin::builders::definitions::PhpFunctionDef;
    use crate::plugin::context::PluginDecoratorDef;
    use crate::plugin::handler::{PluginInternalHandler, PluginMetricsCollector};
    use crate::plugin::php::PluginNativeFunctionDef;
    use std::collections::HashMap;

    fn make_context_and_functions(enabled: bool) -> Vec<PhpFunctionDef> {
        let mut dispatcher = EventDispatcher::new();
        let mut services: HashMap<String, Box<dyn std::any::Any + Send + Sync>> = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics_collectors: Vec<Box<dyn PluginMetricsCollector>> = Vec::new();
        let mut internal_routes: HashMap<String, Box<dyn PluginInternalHandler>> = HashMap::new();
        let mut internal_route_prefixes: Vec<(String, Box<dyn PluginInternalHandler>)> = Vec::new();
        let mut native_php_functions: Vec<PluginNativeFunctionDef> = Vec::new();
        let mut decorators: Vec<PluginDecoratorDef> = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions: Vec<PhpFunctionDef> = Vec::new();
        let mut core_flags: HashMap<String, String> = HashMap::new();

        let mut ctx = PluginContext::new(
            "apm".into(),
            "__oxp_apm_".into(),
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics_collectors,
            &mut internal_routes,
            &mut internal_route_prefixes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
            &mut core_flags,
        );
        register_functions(&mut ctx, enabled).unwrap();
        drop(ctx);
        php_functions
    }

    #[test]
    fn test_registers_all_10_functions() {
        let funcs = make_context_and_functions(true);
        assert_eq!(funcs.len(), 10);
    }

    #[test]
    fn test_function_names_are_exact() {
        let funcs = make_context_and_functions(true);
        let names: Vec<&str> = funcs.iter().map(|f| f.fqn.as_str()).collect();
        assert!(names.contains(&"oxphp_apm_trace"));
        assert!(names.contains(&"oxphp_apm_start"));
        assert!(names.contains(&"oxphp_apm_end"));
        assert!(names.contains(&"oxphp_apm_attribute"));
        assert!(names.contains(&"oxphp_apm_event"));
        assert!(names.contains(&"oxphp_apm_error"));
        assert!(names.contains(&"oxphp_apm_status"));
        assert!(names.contains(&"oxphp_apm_trace_id"));
        assert!(names.contains(&"oxphp_apm_span_id"));
        assert!(names.contains(&"oxphp_apm_header"));
    }

    #[test]
    fn test_all_functions_belong_to_apm_plugin() {
        let funcs = make_context_and_functions(true);
        for func in &funcs {
            assert_eq!(func.plugin_name, "apm");
        }
    }

    #[test]
    fn test_registers_same_count_when_disabled() {
        let funcs = make_context_and_functions(false);
        // All 10 are registered even when disabled
        assert_eq!(funcs.len(), 10);
    }

    #[test]
    fn test_trace_start_param_signature() {
        let funcs = make_context_and_functions(true);
        let f = funcs.iter().find(|f| f.fqn == "oxphp_apm_start").unwrap();
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name, "name");
        assert!(f.params[0].required);
        assert_eq!(f.params[1].name, "attributes");
        assert!(!f.params[1].required);
    }

    #[test]
    fn test_trace_end_param_signature() {
        let funcs = make_context_and_functions(true);
        let f = funcs.iter().find(|f| f.fqn == "oxphp_apm_end").unwrap();
        assert_eq!(f.params.len(), 1);
        assert_eq!(f.params[0].name, "span_id");
        assert!(f.params[0].required);
    }

    #[test]
    fn test_trace_attribute_param_signature() {
        let funcs = make_context_and_functions(true);
        let f = funcs
            .iter()
            .find(|f| f.fqn == "oxphp_apm_attribute")
            .unwrap();
        assert_eq!(f.params.len(), 3);
        assert_eq!(f.params[0].name, "key");
        assert!(f.params[0].required);
        assert_eq!(f.params[1].name, "value");
        assert!(f.params[1].required);
        assert_eq!(f.params[2].name, "span_id");
        assert!(!f.params[2].required);
    }

    #[test]
    fn test_trace_id_no_params() {
        let funcs = make_context_and_functions(true);
        let f = funcs
            .iter()
            .find(|f| f.fqn == "oxphp_apm_trace_id")
            .unwrap();
        assert!(f.params.is_empty());
    }

    #[test]
    fn test_trace_span_id_no_params() {
        let funcs = make_context_and_functions(true);
        let f = funcs.iter().find(|f| f.fqn == "oxphp_apm_span_id").unwrap();
        assert!(f.params.is_empty());
    }

    #[test]
    fn test_trace_header_no_params() {
        let funcs = make_context_and_functions(true);
        let f = funcs.iter().find(|f| f.fqn == "oxphp_apm_header").unwrap();
        assert!(f.params.is_empty());
    }

    #[test]
    fn test_trace_event_param_signature() {
        let funcs = make_context_and_functions(true);
        let f = funcs.iter().find(|f| f.fqn == "oxphp_apm_event").unwrap();
        assert_eq!(f.params.len(), 3);
        assert_eq!(f.params[0].name, "name");
        assert!(f.params[0].required);
        assert_eq!(f.params[1].name, "attributes");
        assert!(!f.params[1].required);
        assert_eq!(f.params[2].name, "span_id");
        assert!(!f.params[2].required);
    }

    #[test]
    fn test_trace_error_param_signature() {
        let funcs = make_context_and_functions(true);
        let f = funcs.iter().find(|f| f.fqn == "oxphp_apm_error").unwrap();
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name, "exception");
        assert!(f.params[0].required);
        assert_eq!(f.params[1].name, "span_id");
        assert!(!f.params[1].required);
    }

    #[test]
    fn test_trace_status_param_signature() {
        let funcs = make_context_and_functions(true);
        let f = funcs.iter().find(|f| f.fqn == "oxphp_apm_status").unwrap();
        assert_eq!(f.params.len(), 3);
        assert_eq!(f.params[0].name, "code");
        assert!(f.params[0].required);
        assert_eq!(f.params[1].name, "description");
        assert!(!f.params[1].required);
        assert_eq!(f.params[2].name, "span_id");
        assert!(!f.params[2].required);
    }

    #[test]
    fn test_trace_param_signature() {
        let funcs = make_context_and_functions(true);
        let f = funcs.iter().find(|f| f.fqn == "oxphp_apm_trace").unwrap();
        assert_eq!(f.params.len(), 3);
        assert_eq!(f.params[0].name, "name");
        assert!(f.params[0].required);
        assert_eq!(f.params[1].name, "callback");
        assert!(f.params[1].required);
        assert_eq!(f.params[2].name, "attributes");
        assert!(!f.params[2].required);
    }

    #[test]
    fn test_return_types() {
        let funcs = make_context_and_functions(true);
        let find = |name: &str| funcs.iter().find(|f| f.fqn == name).unwrap();

        // `mixed`, not `void`: oxphp_apm_trace() hands back whatever the
        // traced callback returned.
        assert_eq!(find("oxphp_apm_trace").return_type, Some(PhpType::Mixed));
        assert_eq!(find("oxphp_apm_start").return_type, Some(PhpType::Int));
        assert_eq!(find("oxphp_apm_end").return_type, Some(PhpType::Void));
        assert_eq!(find("oxphp_apm_attribute").return_type, Some(PhpType::Void));
        assert_eq!(find("oxphp_apm_event").return_type, Some(PhpType::Void));
        assert_eq!(find("oxphp_apm_error").return_type, Some(PhpType::Void));
        assert_eq!(find("oxphp_apm_status").return_type, Some(PhpType::Void));
        assert_eq!(
            find("oxphp_apm_trace_id").return_type,
            Some(PhpType::String)
        );
        assert_eq!(find("oxphp_apm_span_id").return_type, Some(PhpType::String));
        assert_eq!(find("oxphp_apm_header").return_type, Some(PhpType::String));
    }

    #[test]
    fn test_param_types() {
        let funcs = make_context_and_functions(true);
        let find = |name: &str| funcs.iter().find(|f| f.fqn == name).unwrap();

        let start = find("oxphp_apm_start");
        assert_eq!(start.params[0].php_type, PhpType::String);
        assert_eq!(start.params[1].php_type, PhpType::Array);

        let end = find("oxphp_apm_end");
        assert_eq!(end.params[0].php_type, PhpType::Int);
    }
}
