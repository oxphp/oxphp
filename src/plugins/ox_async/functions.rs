//! Handler implementations for the 5 async PHP functions:
//!
//! - `oxphp_async(Closure $fn, mixed ...$args): int`
//! - `oxphp_async_await(int $promise_id, float $timeout = 0.0): mixed`
//! - `oxphp_async_await_all(array $promise_ids, float $timeout = 0.0): array`
//! - `oxphp_async_await_race(array $promise_ids, float $timeout = 0.0): array`
//! - `oxphp_async_await_any(array $promise_ids, float $timeout = 0.0): array`

use std::ffi::CStr;
use std::os::raw::c_void;

use crate::bridge::call::{NativeCall, OwnedResult};
use crate::bridge::{ffi, types::ValType};
use crate::plugin::types::{PhpType, PhpValue};
use crate::plugin::{PhpError, PluginContext, PluginError};

// ─── Constants ───────────────────────────────────────────────────────────────

const DISABLED_MSG: &str = "Async pool is disabled. Set ASYNC_WORKERS > 0 to enable.";
const EXCEPTION_CLASS: &str = "OxPHP\\Async\\AsyncException";
const TIMEOUT_CLASS: &str = "OxPHP\\Async\\TimeoutException";

// ─── Error helpers ───────────────────────────────────────────────────────────

/// Build a "pool disabled" exception.
fn async_disabled() -> PhpError {
    PhpError::Exception {
        class: EXCEPTION_CLASS.to_string(),
        message: DISABLED_MSG.to_string(),
        code: 0,
    }
}

/// Build a generic async exception.
fn async_err(message: impl Into<String>) -> PhpError {
    PhpError::Exception {
        class: EXCEPTION_CLASS.to_string(),
        message: message.into(),
        code: 0,
    }
}

/// Build a timeout exception.
fn timeout_err(message: impl Into<String>) -> PhpError {
    PhpError::Exception {
        class: TIMEOUT_CLASS.to_string(),
        message: message.into(),
        code: 0,
    }
}

/// Read the async exception stored in bridge TLS (set by worker thread on failure).
/// Returns a `PhpError::Exception` wrapping the original exception details inside
/// an `OxPHP\Async\AsyncException` — matching the old C behavior:
/// `"Async task failed: [OriginalClass] original message"`.
fn read_bridge_exception() -> PhpError {
    unsafe {
        let cls_ptr = ffi::oxphp_bridge_get_async_exc_class();
        let msg_ptr = ffi::oxphp_bridge_get_async_exc_message();

        let orig_class = if cls_ptr.is_null() {
            "Unknown"
        } else {
            CStr::from_ptr(cls_ptr).to_str().unwrap_or("Unknown")
        };

        let orig_msg = if msg_ptr.is_null() {
            "unknown error"
        } else {
            CStr::from_ptr(msg_ptr).to_str().unwrap_or("unknown error")
        };

        let message = format!("Async task failed: [{orig_class}] {orig_msg}");

        ffi::oxphp_bridge_clear_async_exception();

        PhpError::Exception {
            class: EXCEPTION_CLASS.to_string(),
            message,
            code: 0,
        }
    }
}

// ─── Registration ────────────────────────────────────────────────────────────

/// Register the 5 async PHP functions via the builder API.
pub fn register_functions(ctx: &mut PluginContext, enabled: bool) -> Result<(), PluginError> {
    // 1. oxphp_async(Closure $fn, mixed ...$args): int
    ctx.function("oxphp_async")
        .param("fn", PhpType::Callable)
        .variadic_param("args", PhpType::Mixed)
        .returns(PhpType::Int)
        .handler(move |call: &mut NativeCall| handler_async(call, enabled))?;

    // 2. oxphp_async_await(int $promise_id, float $timeout = 0.0): mixed
    ctx.function("oxphp_async_await")
        .param("promise_id", PhpType::Int)
        .optional_param("timeout", PhpType::Float, PhpValue::Float(0.0))
        .returns(PhpType::Mixed)
        .handler(move |call: &mut NativeCall| handler_await(call, enabled))?;

    // 3. oxphp_async_await_all(array $promise_ids, float $timeout = 0.0): array
    ctx.function("oxphp_async_await_all")
        .param("promise_ids", PhpType::Array)
        .optional_param("timeout", PhpType::Float, PhpValue::Float(0.0))
        .returns(PhpType::Array)
        .handler(move |call: &mut NativeCall| handler_await_all(call, enabled))?;

    // 4. oxphp_async_await_race(array $promise_ids, float $timeout = 0.0): array
    ctx.function("oxphp_async_await_race")
        .param("promise_ids", PhpType::Array)
        .optional_param("timeout", PhpType::Float, PhpValue::Float(0.0))
        .returns(PhpType::Array)
        .handler(move |call: &mut NativeCall| handler_await_race(call, enabled))?;

    // 5. oxphp_async_await_any(array $promise_ids, float $timeout = 0.0): array
    ctx.function("oxphp_async_await_any")
        .param("promise_ids", PhpType::Array)
        .optional_param("timeout", PhpType::Float, PhpValue::Float(0.0))
        .returns(PhpType::Array)
        .handler(move |call: &mut NativeCall| handler_await_any(call, enabled))?;

    Ok(())
}

// ─── handler_async ───────────────────────────────────────────────────────────

/// `oxphp_async(Closure $fn, mixed ...$args): int`
///
/// Dispatches a closure + arguments to the async worker pool.
/// Returns a promise ID (positive i64) that can be awaited.
fn handler_async(call: &mut NativeCall, enabled: bool) -> Result<(), PhpError> {
    if !enabled {
        return Err(async_disabled());
    }

    // Nested oxphp_async() from inside an async worker is allowed: the task
    // runs in a scheduler fiber, so an await on the nested promise suspends
    // the fiber and frees the worker to run the nested task (async composition).

    // Get the closure zval (arg 0)
    let closure_zval = unsafe { call.raw_arg_ptr(0) };

    // Extract op_array from the closure
    let op_array = unsafe { ffi::oxphp_closure_get_op_array(closure_zval) };
    if op_array.is_null() {
        return Err(async_err("oxphp_async(): first argument must be a Closure"));
    }

    // Get bound $this (may be null for unbound closures)
    let this_ptr = unsafe { ffi::oxphp_closure_get_this(closure_zval) };

    // Get static vars (use-vars captured by the closure)
    let mut static_vars: *mut c_void = std::ptr::null_mut();
    let sv_rc = unsafe { ffi::oxphp_closure_get_static_vars(closure_zval, &mut static_vars) };
    if sv_rc != 0 {
        return Err(async_err(
            "oxphp_async(): failed to extract closure static variables",
        ));
    }

    // Validate use-vars: reject resources and non-Shareable objects (not safe to cross threads)
    if !static_vars.is_null()
        && unsafe { ffi::oxphp_ht_has_non_shareable_objects(static_vars) != 0 }
    {
        return Err(async_err(
            "oxphp_async(): closure use-vars must not contain resources or non-Shareable objects",
        ));
    }

    // Validate variadic args (indices 1..argc): reject resources and non-Shareable objects
    let argc = call.argc();
    for i in 1..argc {
        let t = call.arg_type(i)?;
        if t == ValType::Resource {
            return Err(async_err(format!(
                "oxphp_async(): argument {} must not be a resource",
                i
            )));
        }
        if t == ValType::Object {
            let arg_ptr = unsafe { call.raw_arg_ptr(i) };
            if unsafe { crate::bridge::ffi::oxphp_is_shareable(arg_ptr as *const c_void) } == 0 {
                return Err(async_err(format!(
                    "oxphp_async(): argument {} is a non-Shareable object; only OxPHP\\Shared\\* instances may cross thread boundary",
                    i
                )));
            }
        }
    }

    // Calculate variadic arg count and pointer
    let variadic_argc = argc.saturating_sub(1);
    let variadic_args = if variadic_argc > 0 {
        unsafe { call.raw_arg_ptr(1) }
    } else {
        std::ptr::null_mut()
    };

    // Dispatch to the async worker pool
    let promise_id = unsafe {
        ffi::oxphp_bridge_async_dispatch(
            op_array,
            static_vars,
            this_ptr,
            variadic_argc,
            variadic_args,
            closure_zval,
        )
    };

    if promise_id < 0 {
        return Err(async_err("oxphp_async(): failed to dispatch async task"));
    }

    call.ret_long(promise_id);
    Ok(())
}

// ─── handler_await ───────────────────────────────────────────────────────────

/// `oxphp_async_await(int $promise_id, float $timeout = 0.0): mixed`
///
/// Awaits a single promise. Tries fiber-based suspend first (non-blocking),
/// falls back to blocking await if not inside a fiber.
fn handler_await(call: &mut NativeCall, enabled: bool) -> Result<(), PhpError> {
    if !enabled {
        return Err(async_disabled());
    }

    let promise_id = call.arg_long(0)?;
    let timeout = if call.argc() > 1 {
        match call.arg_is_null(1) {
            Ok(true) => 0.0,
            Ok(false) => call.arg_double(1).unwrap_or(0.0),
            Err(_) => 0.0,
        }
    } else {
        0.0
    };

    let retval = call.retval_ptr();

    // Try fiber path first: suspends the current fiber while waiting
    let fiber_rc = unsafe { ffi::oxphp_bridge_fiber_await(promise_id, timeout, retval) };

    match fiber_rc {
        // 0 = done via fiber, retval already written
        0 => Ok(()),
        // The fiber is unwinding with an exception already pending in PHP.
        // Return without raising one of our own so the real cause survives.
        ffi::OXPHP_FIBER_UNWIND => Ok(()),
        // 1 = not in a fiber, fall through to blocking path
        1 => {
            let rc = unsafe { ffi::oxphp_bridge_await_dispatch(promise_id, timeout, retval) };
            match rc {
                0 => Ok(()),
                -2 => Err(timeout_err(format!(
                    "oxphp_async_await(): promise {promise_id} timed out"
                ))),
                _ => Err(read_bridge_exception()),
            }
        }
        // -2 = timeout on fiber path
        -2 => Err(timeout_err(format!(
            "oxphp_async_await(): promise {promise_id} timed out"
        ))),
        // -3 = the task was cancelled while suspended (awaiter gave up)
        -3 => Err(async_err("Async task cancelled")),
        // -1 or other = error
        _ => Err(read_bridge_exception()),
    }
}

// ─── handler_await_all ───────────────────────────────────────────────────────

/// `oxphp_async_await_all(array $promise_ids, float $timeout = 0.0): array`
///
/// Awaits all promises in the array. Returns an associative array keyed by
/// promise ID with each promise's result value.
fn handler_await_all(call: &mut NativeCall, enabled: bool) -> Result<(), PhpError> {
    if !enabled {
        return Err(async_disabled());
    }

    // Collect promise IDs from the array argument
    let mut ids: Vec<i64> = Vec::new();
    call.arg_array_foreach(0, |_key, val| {
        if val.val_type() == ValType::Long {
            ids.push(val.as_long());
        }
    })?;

    if ids.is_empty() {
        // Return an empty array
        call.ret_array(0, |_| {});
        return Ok(());
    }

    let timeout = if call.argc() > 1 {
        match call.arg_is_null(1) {
            Ok(true) => 0.0,
            Ok(false) => call.arg_double(1).unwrap_or(0.0),
            Err(_) => 0.0,
        }
    } else {
        0.0
    };

    let retval = call.retval_ptr();
    let count = ids.len() as u32;

    // Initialize the return array
    unsafe { ffi::oxphp_ret_array_init(retval, count) };

    // Await each promise and add its result to the array
    for (i, &pid) in ids.iter().enumerate() {
        // Temporary slot for this promise's result. It owns whatever the await
        // writes into it: adding to the return array below copies (ZVAL_COPY),
        // so the array's reference is its own and this one still has to be
        // released. Leaving that to the drop at the end of the iteration covers
        // the bail-out arms too, where the await can have written a partial
        // value before failing.
        let mut temp = OwnedResult::undef();
        let temp_ptr = temp.as_mut_ptr();

        // Suspend on each promise in turn when inside a task fiber, falling
        // back to a blocking await otherwise. The promises already run
        // concurrently on the pool, so awaiting them sequentially still yields
        // parallel wall-time — and crucially, suspending frees the worker so
        // the awaited (possibly nested) tasks can make progress.
        let mut rc = unsafe { ffi::oxphp_bridge_fiber_await(pid, timeout, temp_ptr) };
        if rc == 1 {
            rc = unsafe { ffi::oxphp_bridge_await_dispatch(pid, timeout, temp_ptr) };
        }

        match rc {
            0 => {
                // Success: add to return array keyed by promise ID.
                unsafe { ffi::oxphp_arr_add_index_zval(retval, pid as u64, temp_ptr) };
            }
            // All-or-nothing bail: this promise timed out, was cancelled, or
            // rejected, so await_all is abandoning the whole set. Cancel and
            // strand the promises from here on (the current one plus any not
            // yet awaited) so CPU-bound members don't keep running unobserved
            // until RSHUTDOWN — matching await_race/await_any. Already-consumed
            // promises in the range are a no-op. ids[..i] already completed.
            -2 => {
                strand_promises(&ids[i..]);
                return Err(timeout_err(format!(
                    "oxphp_async_await_all(): promise {pid} timed out"
                )));
            }
            -3 => {
                strand_promises(&ids[i..]);
                return Err(async_err("Async task cancelled"));
            }
            // Unwinding with an exception already pending: abandon the rest of
            // the set as the other bail arms do, but raise nothing.
            ffi::OXPHP_FIBER_UNWIND => {
                strand_promises(&ids[i..]);
                return Ok(());
            }
            _ => {
                strand_promises(&ids[i..]);
                return Err(read_bridge_exception());
            }
        }
    }

    Ok(())
}

/// Cancel and strand a set of still-pending promises that an `await_all` is
/// abandoning, so their tasks stop running with no observer and RSHUTDOWN can
/// drain them safely. Calling over already-completed/cancelled ids is harmless
/// (each is a no-op once it has left the promise map).
#[cfg(feature = "php")]
fn strand_promises(ids: &[i64]) {
    for &id in ids {
        unsafe { crate::php::sapi::strand_and_cancel_promise(id as u64) };
    }
}

#[cfg(not(feature = "php"))]
fn strand_promises(_ids: &[i64]) {}

// ─── handler_await_race ──────────────────────────────────────────────────────

/// `oxphp_async_await_race(array $promise_ids, float $timeout = 0.0): array`
///
/// Races all promises. Returns `['id' => int, 'value' => mixed]` for the
/// first promise that completes.
fn handler_await_race(call: &mut NativeCall, enabled: bool) -> Result<(), PhpError> {
    if !enabled {
        return Err(async_disabled());
    }

    // Collect promise IDs from the array argument
    let mut ids: Vec<i64> = Vec::new();
    call.arg_array_foreach(0, |_key, val| {
        if val.val_type() == ValType::Long {
            ids.push(val.as_long());
        }
    })?;

    if ids.is_empty() {
        return Err(async_err(
            "oxphp_async_await_race(): promise_ids array must not be empty",
        ));
    }

    let timeout = if call.argc() > 1 {
        match call.arg_is_null(1) {
            Ok(true) => 0.0,
            Ok(false) => call.arg_double(1).unwrap_or(0.0),
            Err(_) => 0.0,
        }
    } else {
        0.0
    };

    let count = ids.len() as u32;

    // Temporary slot for the winner's result. It owns whatever the dispatch
    // writes into it: adding to the return array below copies (ZVAL_COPY), so
    // the array's reference is its own and this one still has to be released.
    // Leaving that to the drop covers the error arms too, where the dispatch
    // can have written a partial value before failing.
    let mut winner_zval = OwnedResult::undef();
    let winner_ptr = winner_zval.as_mut_ptr();
    let mut winner_id: i64 = -1;

    let rc = unsafe {
        ffi::oxphp_bridge_await_race_dispatch(
            ids.as_ptr(),
            count,
            timeout,
            &mut winner_id,
            winner_ptr,
        )
    };

    match rc {
        0 => {
            // Build return array: ['id' => winner_id, 'value' => winner_result]
            let retval = call.retval_ptr();
            unsafe { ffi::oxphp_ret_array_init(retval, 2) };
            unsafe {
                ffi::oxphp_arr_add_long(
                    retval,
                    c"id".as_ptr(),
                    2, // key length without NUL
                    winner_id,
                )
            };
            unsafe { ffi::oxphp_arr_add_zval(retval, c"value".as_ptr(), winner_ptr) };
            Ok(())
        }
        -2 => Err(timeout_err(
            "oxphp_async_await_race(): all promises timed out",
        )),
        -4 => Err(async_err(format!(
            "oxphp_async_await_race(): unknown or already-awaited promise id {}",
            winner_id
        ))),
        _ => Err(read_bridge_exception()),
    }
}

// ─── handler_await_any ───────────────────────────────────────────────────────

/// `oxphp_async_await_any(array $promise_ids, ?float $timeout = null): array`
///
/// Promise.any semantics: first FULFILLED promise wins. Rejections accumulate.
/// If every promise rejects, the bridge throws OxPHP\Async\AggregateAsyncException
/// carrying every error. If the timeout expires before any promise fulfills, the
/// bridge throws OxPHP\Async\TimeoutException populated with partial errors and
/// the ids of promises still pending (now cancelled).
fn handler_await_any(call: &mut NativeCall, enabled: bool) -> Result<(), PhpError> {
    if !enabled {
        return Err(async_disabled());
    }

    // Collect promise IDs from the array argument
    let mut ids: Vec<i64> = Vec::new();
    call.arg_array_foreach(0, |_key, val| {
        if val.val_type() == ValType::Long {
            ids.push(val.as_long());
        }
    })?;

    if ids.is_empty() {
        return Err(async_err(
            "oxphp_async_await_any(): promise_ids array must not be empty",
        ));
    }

    let timeout = if call.argc() > 1 {
        match call.arg_is_null(1) {
            Ok(true) => 0.0,
            Ok(false) => call.arg_double(1).unwrap_or(0.0),
            Err(_) => 0.0,
        }
    } else {
        0.0
    };

    let count = ids.len() as u32;

    // Temporary slot for the winner's result. It owns whatever the dispatch
    // writes into it: adding to the return array below copies (ZVAL_COPY), so
    // the array's reference is its own and this one still has to be released.
    // Leaving that to the drop covers the error arms too, where the dispatch
    // can have written a partial value before failing.
    let mut winner_zval = OwnedResult::undef();
    let winner_ptr = winner_zval.as_mut_ptr();
    let mut winner_id: i64 = -1;

    let rc = unsafe {
        ffi::oxphp_bridge_await_any_dispatch(
            ids.as_ptr(),
            count,
            timeout,
            &mut winner_id,
            winner_ptr,
        )
    };

    match rc {
        0 => {
            // Build return array: ['id' => winner_id, 'value' => winner_result]
            let retval = call.retval_ptr();
            unsafe { ffi::oxphp_ret_array_init(retval, 2) };
            unsafe {
                ffi::oxphp_arr_add_long(
                    retval,
                    c"id".as_ptr(),
                    2, // key length without NUL
                    winner_id,
                )
            };
            unsafe { ffi::oxphp_arr_add_zval(retval, c"value".as_ptr(), winner_ptr) };
            Ok(())
        }
        // -2 → bridge threw OxPHP\Async\TimeoutException via aggregate API
        // -3 → bridge threw OxPHP\Async\AggregateAsyncException
        // EG(exception) is already set by zend_throw_exception_object inside the
        // bridge. Returning Ok(()) lets Zend's normal unwinding propagate that
        // pre-set exception unchanged. Returning Err(...) would route through
        // oxphp_throw_exception in the outer SAPI dispatch, which calls
        // zend_throw_exception unconditionally, OVERRIDING EG(exception) with
        // a generic AsyncException. Don't do that.
        -2 | -3 => Ok(()),
        -4 => Err(async_err(format!(
            "oxphp_async_await_any(): unknown or already-awaited promise id {}",
            winner_id
        ))),
        _ => Err(read_bridge_exception()),
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

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
            "async".into(),
            "__oxp_async_".into(),
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
    fn test_registers_all_5_functions() {
        let funcs = make_context_and_functions(true);
        assert_eq!(funcs.len(), 5);
    }

    #[test]
    fn test_function_names_are_exact() {
        let funcs = make_context_and_functions(true);
        let names: Vec<&str> = funcs.iter().map(|f| f.fqn.as_str()).collect();
        assert!(names.contains(&"oxphp_async"));
        assert!(names.contains(&"oxphp_async_await"));
        assert!(names.contains(&"oxphp_async_await_all"));
        assert!(names.contains(&"oxphp_async_await_race"));
        assert!(names.contains(&"oxphp_async_await_any"));
    }

    #[test]
    fn test_all_functions_belong_to_async_plugin() {
        let funcs = make_context_and_functions(true);
        for func in &funcs {
            assert_eq!(func.plugin_name, "async");
        }
    }

    #[test]
    fn test_registers_same_count_when_disabled() {
        let funcs = make_context_and_functions(false);
        assert_eq!(funcs.len(), 5);
    }

    #[test]
    fn test_oxphp_async_param_signature() {
        let funcs = make_context_and_functions(true);
        let f = funcs.iter().find(|f| f.fqn == "oxphp_async").unwrap();
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name, "fn");
        assert!(f.params[0].required);
        assert_eq!(f.params[0].php_type, PhpType::Callable);
        assert_eq!(f.params[1].name, "args");
        assert!(f.params[1].is_variadic);
        assert!(f.is_variadic);
    }

    #[test]
    fn test_oxphp_await_param_signature() {
        let funcs = make_context_and_functions(true);
        let f = funcs.iter().find(|f| f.fqn == "oxphp_async_await").unwrap();
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name, "promise_id");
        assert!(f.params[0].required);
        assert_eq!(f.params[0].php_type, PhpType::Int);
        assert_eq!(f.params[1].name, "timeout");
        assert!(!f.params[1].required);
        assert_eq!(f.params[1].php_type, PhpType::Float);
    }

    #[test]
    fn test_oxphp_async_await_all_param_signature() {
        let funcs = make_context_and_functions(true);
        let f = funcs
            .iter()
            .find(|f| f.fqn == "oxphp_async_await_all")
            .unwrap();
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name, "promise_ids");
        assert!(f.params[0].required);
        assert_eq!(f.params[0].php_type, PhpType::Array);
        assert_eq!(f.params[1].name, "timeout");
        assert!(!f.params[1].required);
        assert_eq!(f.params[1].php_type, PhpType::Float);
    }

    #[test]
    fn test_oxphp_async_await_race_param_signature() {
        let funcs = make_context_and_functions(true);
        let f = funcs
            .iter()
            .find(|f| f.fqn == "oxphp_async_await_race")
            .unwrap();
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name, "promise_ids");
        assert!(f.params[0].required);
        assert_eq!(f.params[0].php_type, PhpType::Array);
        assert_eq!(f.params[1].name, "timeout");
        assert!(!f.params[1].required);
        assert_eq!(f.params[1].php_type, PhpType::Float);
    }

    #[test]
    fn test_oxphp_async_await_any_param_signature() {
        let funcs = make_context_and_functions(true);
        let f = funcs
            .iter()
            .find(|f| f.fqn == "oxphp_async_await_any")
            .unwrap();
        assert_eq!(f.params.len(), 2);
        assert_eq!(f.params[0].name, "promise_ids");
        assert!(f.params[0].required);
        assert_eq!(f.params[0].php_type, PhpType::Array);
        assert_eq!(f.params[1].name, "timeout");
        assert!(!f.params[1].required);
        assert_eq!(f.params[1].php_type, PhpType::Float);
    }

    #[test]
    fn test_return_types() {
        let funcs = make_context_and_functions(true);
        let find = |name: &str| funcs.iter().find(|f| f.fqn == name).unwrap();

        assert_eq!(find("oxphp_async").return_type, Some(PhpType::Int));
        assert_eq!(find("oxphp_async_await").return_type, Some(PhpType::Mixed));
        assert_eq!(
            find("oxphp_async_await_all").return_type,
            Some(PhpType::Array)
        );
        assert_eq!(
            find("oxphp_async_await_race").return_type,
            Some(PhpType::Array)
        );
        assert_eq!(
            find("oxphp_async_await_any").return_type,
            Some(PhpType::Array)
        );
    }

    #[test]
    fn test_async_disabled_error() {
        let err = async_disabled();
        match err {
            PhpError::Exception {
                ref class,
                ref message,
                code,
            } => {
                assert_eq!(class, EXCEPTION_CLASS);
                assert_eq!(message, DISABLED_MSG);
                assert_eq!(code, 0);
            }
            _ => panic!("Expected PhpError::Exception"),
        }
    }

    #[test]
    fn test_async_err_helper() {
        let err = async_err("test error");
        match err {
            PhpError::Exception {
                ref class,
                ref message,
                code,
            } => {
                assert_eq!(class, EXCEPTION_CLASS);
                assert_eq!(message, "test error");
                assert_eq!(code, 0);
            }
            _ => panic!("Expected PhpError::Exception"),
        }
    }

    #[test]
    fn test_timeout_err_helper() {
        let err = timeout_err("timed out");
        match err {
            PhpError::Exception {
                ref class,
                ref message,
                code,
            } => {
                assert_eq!(class, TIMEOUT_CLASS);
                assert_eq!(message, "timed out");
                assert_eq!(code, 0);
            }
            _ => panic!("Expected PhpError::Exception"),
        }
    }

    #[test]
    fn test_read_bridge_exception_with_null_ptrs() {
        // Mock FFI returns null pointers — should get default class/message.
        // The read path also appends the bridge's last-error via
        // oxphp_exception_get, producing a composite message.
        let err = read_bridge_exception();
        match err {
            PhpError::Exception {
                ref class,
                ref message,
                code,
            } => {
                assert_eq!(class, EXCEPTION_CLASS);
                assert!(
                    message.starts_with("Async task failed"),
                    "unexpected message prefix: {message:?}"
                );
                assert_eq!(code, 0);
            }
            _ => panic!("Expected PhpError::Exception"),
        }
    }

    #[test]
    fn test_constants() {
        assert_eq!(
            DISABLED_MSG,
            "Async pool is disabled. Set ASYNC_WORKERS > 0 to enable."
        );
        assert_eq!(EXCEPTION_CLASS, "OxPHP\\Async\\AsyncException");
        assert_eq!(TIMEOUT_CLASS, "OxPHP\\Async\\TimeoutException");
    }

    #[test]
    fn test_all_handlers_have_closures() {
        let funcs = make_context_and_functions(true);
        for func in &funcs {
            assert!(
                func.handler.is_some(),
                "function '{}' should have a handler",
                func.fqn
            );
        }
    }

    #[test]
    fn test_oxphp_async_is_variadic() {
        let funcs = make_context_and_functions(true);
        let f = funcs.iter().find(|f| f.fqn == "oxphp_async").unwrap();
        assert!(f.is_variadic);
    }

    #[test]
    fn test_oxphp_await_not_variadic() {
        let funcs = make_context_and_functions(true);
        let f = funcs.iter().find(|f| f.fqn == "oxphp_async_await").unwrap();
        assert!(!f.is_variadic);
    }

    #[test]
    fn test_oxphp_async_await_all_not_variadic() {
        let funcs = make_context_and_functions(true);
        let f = funcs
            .iter()
            .find(|f| f.fqn == "oxphp_async_await_all")
            .unwrap();
        assert!(!f.is_variadic);
    }

    #[test]
    fn test_oxphp_async_await_race_not_variadic() {
        let funcs = make_context_and_functions(true);
        let f = funcs
            .iter()
            .find(|f| f.fqn == "oxphp_async_await_race")
            .unwrap();
        assert!(!f.is_variadic);
    }

    #[test]
    fn test_oxphp_async_await_any_not_variadic() {
        let funcs = make_context_and_functions(true);
        let f = funcs
            .iter()
            .find(|f| f.fqn == "oxphp_async_await_any")
            .unwrap();
        assert!(!f.is_variadic);
    }
}
