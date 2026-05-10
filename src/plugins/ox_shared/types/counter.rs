//! Shared\Counter — lock-free atomic i64.

use std::os::raw::c_int;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use crate::plugins::ox_shared::error::{
    ffi_entry, read_last_error_message, set_last_error, SharedError,
};
use crate::plugins::ox_shared::registry::{registry, SharedInner, SharedType};
use crate::plugins::ox_shared::value::SharedValue;

pub struct CounterInner {
    value: AtomicI64,
}

impl CounterInner {
    pub fn new(initial: i64) -> Self {
        Self {
            value: AtomicI64::new(initial),
        }
    }

    pub fn get(&self) -> i64 {
        self.value.load(Ordering::SeqCst)
    }

    pub fn swap(&self, v: i64) -> i64 {
        self.value.swap(v, Ordering::SeqCst)
    }

    /// Returns the NEW value.
    pub fn add(&self, delta: i64) -> i64 {
        self.value
            .fetch_add(delta, Ordering::SeqCst)
            .wrapping_add(delta)
    }

    pub fn add_batch(&self, deltas: &[i64]) -> i64 {
        let sum: i64 = deltas.iter().fold(0i64, |acc, d| acc.wrapping_add(*d));
        self.add(sum)
    }
}

impl SharedInner for CounterInner {
    fn type_tag(&self) -> SharedType {
        SharedType::Counter
    }
    fn debug_snapshot(&self) -> SharedValue {
        SharedValue::Long(self.get())
    }
    fn mem_bytes(&self) -> usize {
        16
    }
    fn on_drop(&self) {}
    fn on_shutdown_notify(&self) {}
}

// ─── FFI ──────────────────────────────────────────────────────────────

/// # Safety
/// `out_id` must be valid for writes of `u64` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_counter_create(initial: i64, out_id: *mut u64) -> c_int {
    if out_id.is_null() {
        set_last_error("out_id is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let inner = Arc::new(CounterInner::new(initial));
        let id = reg.insert(SharedType::Counter, inner)?;
        unsafe { *out_id = id };
        Ok(())
    })
}

/// # Safety
/// `out` must be valid for writes of `i64` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_counter_get(id: u64, out: *mut i64) -> c_int {
    if out.is_null() {
        set_last_error("out is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_counter().ok_or(SharedError::Type)?;
        let v = inner.get();
        reg.record_op(id);
        unsafe { *out = v };
        Ok(())
    })
}

/// # Safety
/// `out_prev` must be valid for writes of `i64` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_counter_swap(
    id: u64,
    new_val: i64,
    out_prev: *mut i64,
) -> c_int {
    if out_prev.is_null() {
        set_last_error("out_prev is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_counter().ok_or(SharedError::Type)?;
        let prev = inner.swap(new_val);
        reg.record_op(id);
        unsafe { *out_prev = prev };
        Ok(())
    })
}

/// # Safety
/// `out_new` must be valid for writes of `i64` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_counter_add(id: u64, delta: i64, out_new: *mut i64) -> c_int {
    if out_new.is_null() {
        set_last_error("out_new is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_counter().ok_or(SharedError::Type)?;
        let new_val = inner.add(delta);
        reg.record_op(id);
        unsafe { *out_new = new_val };
        Ok(())
    })
}

/// # Safety
/// `out_new` must be valid for writes of `i64`. `deltas` must be valid for
/// reads of `n` `i64` values when `n > 0`.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_counter_add_batch(
    id: u64,
    deltas: *const i64,
    n: usize,
    out_new: *mut i64,
) -> c_int {
    if out_new.is_null() || (n > 0 && deltas.is_null()) {
        set_last_error("null argument");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_counter().ok_or(SharedError::Type)?;
        let slice = if n == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(deltas, n) }
        };
        let new_val = inner.add_batch(slice);
        reg.record_op(id);
        unsafe { *out_new = new_val };
        Ok(())
    })
}

// Helper trait for downcasting Arc<dyn SharedInner> to &CounterInner.
// Implemented on `dyn SharedInner` (not `+ Send + Sync`) to match Entry.inner's
// actual trait-object type.
pub trait SharedInnerCounterExt {
    fn as_any_counter(&self) -> Option<&CounterInner>;
}

impl SharedInnerCounterExt for dyn SharedInner {
    fn as_any_counter(&self) -> Option<&CounterInner> {
        if self.type_tag() == SharedType::Counter {
            // SAFETY: type_tag() == Counter guarantees the concrete type is
            // CounterInner. Casting a `*const dyn SharedInner` fat pointer to
            // `*const CounterInner` yields the data pointer, which is the
            // address of the CounterInner allocation. Sound as long as
            // SharedType::Counter is only ever used with CounterInner — enforced
            // by the sole insertion site in oxphp_shared_counter_create.
            Some(unsafe { &*(self as *const dyn SharedInner as *const CounterInner) })
        } else {
            None
        }
    }
}

// ─── PHP class registration ───────────────────────────────────────────

use crate::bridge::types::ValType;
use crate::plugin::types::{MagicMethod, PhpType, PhpValue};
use crate::plugin::{PhpError, PluginContext, PluginError};
use crate::plugins::ox_shared::handle::SharedHandle;

pub fn register_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_class("OxPHP\\Shared\\Counter")
        .implements("OxPHP\\Shared\\Shareable")
        .with_storage(|| SharedHandle::new(SharedType::Counter))
        .magic(MagicMethod::Clone)
        .handler(|_call| {
            Err(PhpError::Exception {
                class: "OxPHP\\Shared\\SharedException".to_string(),
                message: "Shared instances cannot be cloned. Use cross-thread \
                          transfer via oxphp_async(fn() use ($this) {...}) for \
                          sharing, or explicitly create a new instance for an \
                          independent copy."
                    .to_string(),
                code: 0,
            })
        })
        .method("__construct")
        .optional_param("initial", PhpType::Int, PhpValue::Int(0))
        .handler(|call| {
            let initial = if call.argc() > 0 {
                call.arg_long(0).unwrap_or(0)
            } else {
                0
            };
            let mut out_id: u64 = 0;
            let rc = unsafe { oxphp_shared_counter_create(initial, &mut out_id) };
            if rc != 0 {
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\SharedException".to_string(),
                    message: read_last_error_message(),
                    code: 0,
                });
            }
            let handle = call.storage_mut::<SharedHandle>()?;
            handle.shared_id = out_id;
            handle.type_tag = SharedType::Counter as u8;
            Ok(())
        })
        .method("get")
        .returns(PhpType::Int)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let mut out: i64 = 0;
            let rc = unsafe { oxphp_shared_counter_get(handle.shared_id, &mut out) };
            counter_rc_to_result(rc)?;
            call.ret_long(out);
            Ok(())
        })
        .method("inc")
        .optional_param("by", PhpType::Int, PhpValue::Int(1))
        .returns(PhpType::Int)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let by = if call.argc() > 0 {
                call.arg_long(0).unwrap_or(1)
            } else {
                1
            };
            let mut new_val: i64 = 0;
            let rc = unsafe { oxphp_shared_counter_add(handle.shared_id, by, &mut new_val) };
            counter_rc_to_result(rc)?;
            call.ret_long(new_val);
            Ok(())
        })
        .method("dec")
        .optional_param("by", PhpType::Int, PhpValue::Int(1))
        .returns(PhpType::Int)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let by = if call.argc() > 0 {
                call.arg_long(0).unwrap_or(1)
            } else {
                1
            };
            let mut new_val: i64 = 0;
            let rc = unsafe { oxphp_shared_counter_add(handle.shared_id, -by, &mut new_val) };
            counter_rc_to_result(rc)?;
            call.ret_long(new_val);
            Ok(())
        })
        .method("add")
        .param("delta", PhpType::Int)
        .returns(PhpType::Int)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let delta = call.arg_long(0)?;
            let mut new_val: i64 = 0;
            let rc = unsafe { oxphp_shared_counter_add(handle.shared_id, delta, &mut new_val) };
            counter_rc_to_result(rc)?;
            call.ret_long(new_val);
            Ok(())
        })
        .method("addBatch")
        .param("deltas", PhpType::Array)
        .returns(PhpType::Int)
        .handler(|call| {
            let handle_id = call.storage::<SharedHandle>()?.shared_id;
            let mut deltas: Vec<i64> = Vec::new();
            call.arg_array_foreach(0, |_key, val| {
                if val.val_type() == ValType::Long {
                    deltas.push(val.as_long());
                }
            })?;
            let mut new_val: i64 = 0;
            let rc = unsafe {
                oxphp_shared_counter_add_batch(
                    handle_id,
                    deltas.as_ptr(),
                    deltas.len(),
                    &mut new_val,
                )
            };
            counter_rc_to_result(rc)?;
            call.ret_long(new_val);
            Ok(())
        })
        .method("reset")
        .returns(PhpType::Int)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let mut prev: i64 = 0;
            let rc = unsafe { oxphp_shared_counter_swap(handle.shared_id, 0, &mut prev) };
            counter_rc_to_result(rc)?;
            call.ret_long(prev);
            Ok(())
        })
        .method("id")
        .returns(PhpType::Int)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            if !handle.is_initialized() {
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\UninitializedException".to_string(),
                    message: "uninitialised Shared wrapper".to_string(),
                    code: 0,
                });
            }
            call.ret_long(handle.shared_id as i64);
            Ok(())
        })
        .build()?;

    Ok(())
}

pub(super) fn counter_rc_to_result(rc: c_int) -> Result<(), PhpError> {
    if rc == 0 {
        return Ok(());
    }
    let msg = read_last_error_message();
    let class = match rc {
        -2 => "OxPHP\\Shared\\StaleHandleException",
        -3 => "OxPHP\\Shared\\TypeException",
        -4 => "OxPHP\\Shared\\CapacityException",
        -10 => "OxPHP\\Shared\\UninitializedException",
        _ => "OxPHP\\Shared\\SharedException",
    };
    Err(PhpError::Exception {
        class: class.to_string(),
        message: msg,
        code: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_swap_add() {
        let c = CounterInner::new(10);
        assert_eq!(c.get(), 10);
        assert_eq!(c.swap(20), 10);
        assert_eq!(c.get(), 20);
        assert_eq!(c.add(5), 25);
        assert_eq!(c.add(-10), 15);
    }

    #[test]
    fn add_batch_sums_correctly() {
        let c = CounterInner::new(0);
        assert_eq!(c.add_batch(&[1, 2, 3, -4, 10]), 12);
        assert_eq!(c.add_batch(&[]), 12);
    }

    #[test]
    fn overflow_wraps() {
        let c = CounterInner::new(i64::MAX);
        let new = c.add(1);
        assert_eq!(new, i64::MIN);
    }
}
