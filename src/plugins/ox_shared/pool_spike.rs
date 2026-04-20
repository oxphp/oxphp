//! Cross-thread fcc invocation spike.
//!
//! Registers three native PHP functions that exercise the C-side
//! spike slot in `ext/bridge/oxphp_bridge.c`. The goal is to answer
//! one question:
//!
//! > Can a `zend_fcall_info_cache` captured on thread A be safely
//! > invoked via `zend_call_known_function` on thread B under ZTS?
//!
//! If YES → `Shared\Pool` can store the factory fcc once at
//! construction and invoke it from any worker thread that needs
//! to mint a resource.
//!
//! If NO (crash, wrong call target, leaked resources) → Pool's
//! factory path must keep the function name plus capture
//! thread-local info, and re-resolve on every invoking thread.
//!
//! Exposed only as diagnostic functions; retained alongside the
//! real Pool FFI while the probe is still useful.

use crate::bridge::call::NativeCall;
use crate::bridge::ffi;
use crate::plugin::types::PhpType;
use crate::plugin::{PhpError, PluginContext, PluginError};

/// Register the three spike functions on the shared plugin.
pub fn register_functions(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.function("oxphp_pool_spike_capture")
        .param("fn", PhpType::Callable)
        .returns(PhpType::Int)
        .handler(handler_capture)?;

    ctx.function("oxphp_pool_spike_invoke")
        .returns(PhpType::Array)
        .handler(handler_invoke)?;

    ctx.function("oxphp_pool_spike_reset")
        .returns(PhpType::Void)
        .handler(handler_reset)?;

    Ok(())
}

fn handler_capture(call: &mut NativeCall) -> Result<(), PhpError> {
    let callable_zv = unsafe { call.raw_arg_ptr(0) };
    let mut tid: u64 = 0;
    let rc = unsafe { ffi::oxphp_pool_spike_capture(callable_zv, &mut tid) };
    if rc != 0 {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".into(),
            message: "oxphp_pool_spike_capture: argument is not callable".into(),
            code: 0,
        });
    }
    call.ret_long(tid as i64);
    Ok(())
}

fn handler_invoke(call: &mut NativeCall) -> Result<(), PhpError> {
    use crate::plugins::ox_shared::value::{portbuf_to_sv, SharedValue};

    let mut captured: u64 = 0;
    let mut current: u64 = 0;
    let mut buf: *mut u8 = std::ptr::null_mut();
    let mut len: usize = 0;
    let rc =
        unsafe { ffi::oxphp_pool_spike_invoke(&mut captured, &mut current, &mut buf, &mut len) };

    if rc == -1 {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\UninitializedException".into(),
            message: "oxphp_pool_spike_invoke: no captured callable — call oxphp_pool_spike_capture first".into(),
            code: 0,
        });
    }
    if rc == -2 {
        // EG(exception) already set by the invoked closure — just surface it.
        return Err(PhpError::Custom(
            "spike-invoked closure threw; exception propagated".into(),
        ));
    }
    if rc != 0 {
        return Err(PhpError::Custom(format!(
            "oxphp_pool_spike_invoke failed with rc={rc}"
        )));
    }

    // Decode the returned portbuf into a SharedValue for inclusion in
    // the associative array we hand back to PHP.
    let result_sv = if buf.is_null() || len == 0 {
        SharedValue::Null
    } else {
        let slice = unsafe { std::slice::from_raw_parts(buf, len) };
        portbuf_to_sv(slice).unwrap_or(SharedValue::Null)
    };
    if !buf.is_null() {
        unsafe { ffi::oxphp_portable_free(buf) };
    }

    // Assemble ['captured_tid' => .., 'current_tid' => .., 'result' => ..]
    // via an intermediate SharedArray → portbuf → deserialise into RETVAL.
    use crate::plugins::ox_shared::value::{sv_to_portbuf, SharedArray};
    use std::sync::Arc;

    let mut arr = SharedArray::default();
    arr.str_keyed.push((
        Arc::from("captured_tid"),
        SharedValue::Long(captured as i64),
    ));
    arr.str_keyed
        .push((Arc::from("current_tid"), SharedValue::Long(current as i64)));
    arr.str_keyed.push((
        Arc::from("cross_thread"),
        SharedValue::Bool(captured != current),
    ));
    arr.str_keyed.push((Arc::from("result"), result_sv));

    let payload = sv_to_portbuf(&SharedValue::Array(Arc::new(arr)));
    let retval = call.retval_ptr();
    let rc = unsafe {
        ffi::oxphp_portable_deserialize(payload.as_ptr(), payload.len(), 1, retval as *mut _)
    };
    if rc != 0 {
        call.ret_null();
    }
    Ok(())
}

fn handler_reset(call: &mut NativeCall) -> Result<(), PhpError> {
    unsafe { ffi::oxphp_pool_spike_reset() };
    call.ret_null();
    Ok(())
}
