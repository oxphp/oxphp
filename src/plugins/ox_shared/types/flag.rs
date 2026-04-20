//! Shared\Flag — atomic bool.
//!
//! Spec: .internal/technical-docs/en/features/shared/21-type-flag.md

use std::os::raw::c_int;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::plugins::ox_shared::error::{ffi_entry, set_last_error, SharedError};
use crate::plugins::ox_shared::registry::{registry, SharedInner, SharedType};
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

    pub fn test(&self) -> bool {
        self.value.load(Ordering::SeqCst)
    }

    /// Sets the flag to true. Returns the previous value.
    pub fn set(&self) -> bool {
        self.value.swap(true, Ordering::SeqCst)
    }

    /// Clears the flag (sets to false). Returns the previous value.
    pub fn clear(&self) -> bool {
        self.value.swap(false, Ordering::SeqCst)
    }

    /// Compare-and-swap. Returns true if the swap was performed.
    pub fn cas(&self, expect: bool, new: bool) -> bool {
        self.value
            .compare_exchange(expect, new, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Exchange: set to `new`, return the previous value.
    pub fn exchange(&self, new: bool) -> bool {
        self.value.swap(new, Ordering::SeqCst)
    }
}

impl SharedInner for FlagInner {
    fn type_tag(&self) -> SharedType {
        SharedType::Flag
    }
    fn debug_snapshot(&self) -> SharedValue {
        SharedValue::Bool(self.test())
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
pub unsafe extern "C" fn oxphp_shared_flag_create(initial: c_int, out_id: *mut u64) -> c_int {
    if out_id.is_null() {
        set_last_error("out_id is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let inner = Arc::new(FlagInner::new(initial != 0));
        let id = reg.insert(SharedType::Flag, inner)?;
        unsafe { *out_id = id };
        Ok(())
    })
}

/// # Safety
/// `out` must be valid for writes of `c_int` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_flag_test(id: u64, out: *mut c_int) -> c_int {
    if out.is_null() {
        set_last_error("out is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_flag().ok_or(SharedError::Type)?;
        let v = inner.test();
        reg.record_op(id);
        unsafe { *out = v as c_int };
        Ok(())
    })
}

/// # Safety
/// `out_prev` must be valid for writes of `c_int` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_flag_set(id: u64, out_prev: *mut c_int) -> c_int {
    if out_prev.is_null() {
        set_last_error("out_prev is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_flag().ok_or(SharedError::Type)?;
        let prev = inner.set();
        reg.record_op(id);
        unsafe { *out_prev = prev as c_int };
        Ok(())
    })
}

/// # Safety
/// `out_prev` must be valid for writes of `c_int` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_flag_clear(id: u64, out_prev: *mut c_int) -> c_int {
    if out_prev.is_null() {
        set_last_error("out_prev is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_flag().ok_or(SharedError::Type)?;
        let prev = inner.clear();
        reg.record_op(id);
        unsafe { *out_prev = prev as c_int };
        Ok(())
    })
}

/// # Safety
/// `out_swapped` must be valid for writes of `c_int` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_flag_cas(
    id: u64,
    expect: c_int,
    new_val: c_int,
    out_swapped: *mut c_int,
) -> c_int {
    if out_swapped.is_null() {
        set_last_error("out_swapped is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_flag().ok_or(SharedError::Type)?;
        let swapped = inner.cas(expect != 0, new_val != 0);
        reg.record_op(id);
        unsafe { *out_swapped = swapped as c_int };
        Ok(())
    })
}

/// # Safety
/// `out_prev` must be valid for writes of `c_int` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_flag_exchange(
    id: u64,
    new_val: c_int,
    out_prev: *mut c_int,
) -> c_int {
    if out_prev.is_null() {
        set_last_error("out_prev is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_flag().ok_or(SharedError::Type)?;
        let prev = inner.exchange(new_val != 0);
        reg.record_op(id);
        unsafe { *out_prev = prev as c_int };
        Ok(())
    })
}

// Helper trait for downcasting Arc<dyn SharedInner> to &FlagInner.
pub trait SharedInnerFlagExt {
    fn as_any_flag(&self) -> Option<&FlagInner>;
}

impl SharedInnerFlagExt for dyn SharedInner {
    fn as_any_flag(&self) -> Option<&FlagInner> {
        if self.type_tag() == SharedType::Flag {
            // SAFETY: type_tag() == Flag guarantees the concrete type is
            // FlagInner. Sound as long as SharedType::Flag is only ever used
            // with FlagInner — enforced by the sole insertion site in
            // oxphp_shared_flag_create.
            Some(unsafe { &*(self as *const dyn SharedInner as *const FlagInner) })
        } else {
            None
        }
    }
}

// ─── PHP class registration ───────────────────────────────────────────

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
                class: "OxPHP\\Shared\\Exception".to_string(),
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
            let mut out_id: u64 = 0;
            let rc = unsafe { oxphp_shared_flag_create(initial as c_int, &mut out_id) };
            if rc != 0 {
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\Exception".to_string(),
                    message: rc_to_phperr_msg(rc),
                    code: 0,
                });
            }
            let handle = call.storage_mut::<SharedHandle>()?;
            handle.shared_id = out_id;
            handle.type_tag = SharedType::Flag as u8;
            Ok(())
        })
        .method("test")
        .returns(PhpType::Bool)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let mut out: c_int = 0;
            let rc = unsafe { oxphp_shared_flag_test(handle.shared_id, &mut out) };
            super::counter::counter_rc_to_result(rc)?;
            call.ret_bool(out != 0);
            Ok(())
        })
        .method("set")
        .returns(PhpType::Bool)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let mut prev: c_int = 0;
            let rc = unsafe { oxphp_shared_flag_set(handle.shared_id, &mut prev) };
            super::counter::counter_rc_to_result(rc)?;
            call.ret_bool(prev != 0);
            Ok(())
        })
        .method("clear")
        .returns(PhpType::Bool)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let mut prev: c_int = 0;
            let rc = unsafe { oxphp_shared_flag_clear(handle.shared_id, &mut prev) };
            super::counter::counter_rc_to_result(rc)?;
            call.ret_bool(prev != 0);
            Ok(())
        })
        .method("compareAndSet")
        .param("expect", PhpType::Bool)
        .param("new", PhpType::Bool)
        .returns(PhpType::Bool)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let expect = call.arg_bool(0)?;
            let new = call.arg_bool(1)?;
            let mut swapped: c_int = 0;
            let rc = unsafe {
                oxphp_shared_flag_cas(
                    handle.shared_id,
                    expect as c_int,
                    new as c_int,
                    &mut swapped,
                )
            };
            super::counter::counter_rc_to_result(rc)?;
            call.ret_bool(swapped != 0);
            Ok(())
        })
        .method("exchange")
        .param("new", PhpType::Bool)
        .returns(PhpType::Bool)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let new = call.arg_bool(0)?;
            let mut prev: c_int = 0;
            let rc =
                unsafe { oxphp_shared_flag_exchange(handle.shared_id, new as c_int, &mut prev) };
            super::counter::counter_rc_to_result(rc)?;
            call.ret_bool(prev != 0);
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

fn rc_to_phperr_msg(rc: c_int) -> String {
    // Reuse counter's error-string extraction via last_error.
    let mut buf = [0u8; 512];
    let n = unsafe {
        crate::plugins::ox_shared::error::oxphp_shared_last_error(
            buf.as_mut_ptr() as *mut std::os::raw::c_char,
            buf.len(),
        )
    };
    let _ = rc; // rc used for context if needed in the future
    let len = n.min(buf.len() - 1);
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_set_clear() {
        let f = FlagInner::new(false);
        // set() when false → prev = false
        assert!(!f.set());
        // set() again when true → prev = true
        assert!(f.set());
        // clear() when true → prev = true
        assert!(f.clear());
        // clear() again when false → prev = false
        assert!(!f.clear());
    }

    #[test]
    fn cas() {
        let f = FlagInner::new(false);
        // cas(false → true) should succeed
        assert!(f.cas(false, true));
        // now flag is true; cas(false → true) should fail
        assert!(!f.cas(false, true));
        // flag is still true
        assert!(f.test());
    }
}
