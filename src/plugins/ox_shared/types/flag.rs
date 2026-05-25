//! Shared\Flag — atomic bool with explicit memory ordering control.
//!
//! The bool twin of `Shared\Atomic`: load/store/swap/compareAndSet, each
//! taking an explicit `Ordering`, validated identically to Atomic.

use std::os::raw::c_int;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::plugins::ox_shared::error::{
    ffi_entry, read_last_error_message, set_last_error, SharedError,
};
use crate::plugins::ox_shared::registry::{registry, Entry, SharedInner, SharedType, ENTRY_MAGIC};
use crate::plugins::ox_shared::value::SharedValue;

pub struct FlagInner {
    value: AtomicBool,
}

impl FlagInner {
    pub fn new(initial: bool) -> Self {
        Self {
            value: AtomicBool::new(initial),
        }
    }

    pub fn load(&self, order: Ordering) -> bool {
        self.value.load(order)
    }

    pub fn store(&self, v: bool, order: Ordering) {
        self.value.store(v, order);
    }

    pub fn swap(&self, v: bool, order: Ordering) -> bool {
        self.value.swap(v, order)
    }

    pub fn compare_and_set(
        &self,
        expect: bool,
        new: bool,
        success: Ordering,
        failure: Ordering,
    ) -> bool {
        self.value
            .compare_exchange(expect, new, success, failure)
            .is_ok()
    }
}

impl SharedInner for FlagInner {
    fn type_tag(&self) -> SharedType {
        SharedType::Flag
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn debug_snapshot(&self) -> SharedValue {
        // Relaxed: introspection-only read, never part of a caller-controlled
        // happens-before — a stronger ordering would add a barrier for nothing.
        SharedValue::Bool(self.load(Ordering::Relaxed))
    }
    fn mem_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn on_drop(&self) {}
    fn on_shutdown_notify(&self) {}
}

// ─── FFI ──────────────────────────────────────────────────────────────

use super::atomic::ordering_from_u8;

/// # Safety
/// `out_ptr` must be valid for writes of `*const Entry` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_flag_create(
    initial: c_int,
    out_ptr: *mut *const Entry,
) -> c_int {
    if out_ptr.is_null() {
        set_last_error("out_ptr is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let inner = Arc::new(FlagInner::new(initial != 0));
        let arc = reg.insert(SharedType::Flag, inner)?;
        // SAFETY: out_ptr checked non-null above.
        unsafe { *out_ptr = Arc::into_raw(arc) };
        Ok(())
    })
}

/// # Safety
/// `entry_ptr` must be a live `Arc::into_raw` pointer or NULL.
/// `out` must be valid for writes of `c_int` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_flag_load(
    entry_ptr: *const Entry,
    order: u8,
    out: *mut c_int,
) -> c_int {
    if out.is_null() {
        set_last_error("out is null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        // SAFETY: entry_ptr non-null and, per the handle contract, a live
        // Arc::into_raw pointer — the calling PHP wrapper holds a strong ref.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "flag_load on freed Entry");
        let inner = entry.inner.as_any_flag().ok_or(SharedError::Type)?;
        let v = inner.load(ordering_from_u8(order));
        entry.registry.record_op(entry);
        unsafe { *out = v as c_int };
        Ok(())
    })
}

/// # Safety
/// `entry_ptr` must be a live `Arc::into_raw` pointer or NULL.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_flag_store(
    entry_ptr: *const Entry,
    value: c_int,
    order: u8,
) -> c_int {
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_flag_load.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "flag_store on freed Entry");
        let inner = entry.inner.as_any_flag().ok_or(SharedError::Type)?;
        inner.store(value != 0, ordering_from_u8(order));
        entry.registry.record_op(entry);
        Ok(())
    })
}

/// # Safety
/// `entry_ptr` must be a live `Arc::into_raw` pointer or NULL.
/// `out_prev` must be valid for writes of `c_int` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_flag_swap(
    entry_ptr: *const Entry,
    value: c_int,
    order: u8,
    out_prev: *mut c_int,
) -> c_int {
    if out_prev.is_null() {
        set_last_error("out_prev is null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_flag_load.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "flag_swap on freed Entry");
        let inner = entry.inner.as_any_flag().ok_or(SharedError::Type)?;
        let prev = inner.swap(value != 0, ordering_from_u8(order));
        entry.registry.record_op(entry);
        unsafe { *out_prev = prev as c_int };
        Ok(())
    })
}

/// # Safety
/// `entry_ptr` must be a live `Arc::into_raw` pointer or NULL.
/// `out_swapped` must be valid for writes of `c_int` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_flag_cas(
    entry_ptr: *const Entry,
    expect: c_int,
    new_val: c_int,
    success: u8,
    failure: u8,
    out_swapped: *mut c_int,
) -> c_int {
    if out_swapped.is_null() {
        set_last_error("out_swapped is null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_flag_load.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "flag_cas on freed Entry");
        let inner = entry.inner.as_any_flag().ok_or(SharedError::Type)?;
        let swapped = inner.compare_and_set(
            expect != 0,
            new_val != 0,
            ordering_from_u8(success),
            ordering_from_u8(failure),
        );
        entry.registry.record_op(entry);
        unsafe { *out_swapped = swapped as c_int };
        Ok(())
    })
}

// Helper trait for downcasting Arc<dyn SharedInner> to &FlagInner.
pub trait SharedInnerFlagExt {
    fn as_any_flag(&self) -> Option<&FlagInner>;
}

impl SharedInnerFlagExt for dyn SharedInner {
    fn as_any_flag(&self) -> Option<&FlagInner> {
        self.as_any().downcast_ref::<FlagInner>()
    }
}

// ─── PHP class registration ───────────────────────────────────────────

use super::atomic::{
    atomic_rc_to_result, order_default, order_type, read_order_arg, validate_cas_failure_ordering,
    validate_ordering_for_load, validate_ordering_for_store,
};
use crate::plugin::types::{MagicMethod, PhpType, PhpValue};
use crate::plugin::{PhpError, PluginContext, PluginError};
use crate::plugins::ox_shared::handle::SharedHandle;

pub fn register_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_class("OxPHP\\Shared\\Flag")
        .implements("OxPHP\\Shared\\Shareable")
        .with_storage(|| SharedHandle::new(SharedType::Flag))
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
        .optional_param("initial", PhpType::Bool, PhpValue::Bool(false))
        .handler(|call| {
            let initial = if call.argc() > 0 {
                call.arg_bool(0).unwrap_or(false)
            } else {
                false
            };
            let mut out_ptr: *const Entry = std::ptr::null();
            let rc = unsafe { oxphp_shared_flag_create(initial as c_int, &mut out_ptr) };
            if rc != 0 {
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\SharedException".to_string(),
                    message: read_last_error_message(),
                    code: 0,
                });
            }
            let handle = call.storage_mut::<SharedHandle>()?;
            handle.entry_ptr = out_ptr;
            handle.type_tag = SharedType::Flag as u8;
            Ok(())
        })
        .method("load")
        .optional_param("order", order_type(), order_default())
        .returns(PhpType::Bool)
        .handler(|call| {
            let order = read_order_arg(call, 0)?;
            validate_ordering_for_load(order)?;
            let handle = call.storage::<SharedHandle>()?;
            let mut out: c_int = 0;
            let rc = unsafe { oxphp_shared_flag_load(handle.entry_ptr, order, &mut out) };
            atomic_rc_to_result(rc)?;
            call.ret_bool(out != 0);
            Ok(())
        })
        .method("store")
        .param("value", PhpType::Bool)
        .optional_param("order", order_type(), order_default())
        .returns(PhpType::Void)
        .handler(|call| {
            let value = call.arg_bool(0)?;
            let order = read_order_arg(call, 1)?;
            validate_ordering_for_store(order)?;
            let handle = call.storage::<SharedHandle>()?;
            let rc = unsafe { oxphp_shared_flag_store(handle.entry_ptr, value as c_int, order) };
            atomic_rc_to_result(rc)?;
            Ok(())
        })
        .method("swap")
        .param("value", PhpType::Bool)
        .optional_param("order", order_type(), order_default())
        .returns(PhpType::Bool)
        .handler(|call| {
            let value = call.arg_bool(0)?;
            let order = read_order_arg(call, 1)?;
            let handle = call.storage::<SharedHandle>()?;
            let mut prev: c_int = 0;
            let rc = unsafe {
                oxphp_shared_flag_swap(handle.entry_ptr, value as c_int, order, &mut prev)
            };
            atomic_rc_to_result(rc)?;
            call.ret_bool(prev != 0);
            Ok(())
        })
        .method("compareAndSet")
        .param("expect", PhpType::Bool)
        .param("new", PhpType::Bool)
        .optional_param("success", order_type(), order_default())
        .optional_param("failure", order_type(), order_default())
        .returns(PhpType::Bool)
        .handler(|call| {
            let expect = call.arg_bool(0)?;
            let new_val = call.arg_bool(1)?;
            let success = read_order_arg(call, 2)?;
            let failure = read_order_arg(call, 3)?;
            validate_cas_failure_ordering(failure)?;
            let handle = call.storage::<SharedHandle>()?;
            let mut swapped: c_int = 0;
            let rc = unsafe {
                oxphp_shared_flag_cas(
                    handle.entry_ptr,
                    expect as c_int,
                    new_val as c_int,
                    success,
                    failure,
                    &mut swapped,
                )
            };
            atomic_rc_to_result(rc)?;
            call.ret_bool(swapped != 0);
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
            let id = unsafe {
                crate::plugins::ox_shared::registry::oxphp_shared_entry_id(handle.entry_ptr)
            };
            call.ret_long(id as i64);
            Ok(())
        })
        .build()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_store_swap_baseline() {
        let f = FlagInner::new(false);
        assert!(!f.load(Ordering::SeqCst));
        f.store(true, Ordering::SeqCst);
        assert!(f.load(Ordering::Acquire));
        assert!(f.swap(false, Ordering::SeqCst)); // returns previous = true
        assert!(!f.load(Ordering::SeqCst));
    }

    #[test]
    fn cas_success_and_failure_paths() {
        let f = FlagInner::new(false);
        assert!(f.compare_and_set(false, true, Ordering::SeqCst, Ordering::SeqCst));
        assert!(!f.compare_and_set(false, true, Ordering::SeqCst, Ordering::Acquire));
        assert!(f.load(Ordering::SeqCst));
    }
}
