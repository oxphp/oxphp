//! Shared\Counter — lock-free atomic i64.

use std::os::raw::c_int;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;

use crate::plugins::ox_shared::error::{
    ffi_entry, read_last_error_message, set_last_error, SharedError,
};
use crate::plugins::ox_shared::registry::{registry, Entry, SharedInner, SharedType, ENTRY_MAGIC};
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

    // All Counter ops use Relaxed ordering. A Counter is a statistics
    // accumulator, not a synchronisation point: Relaxed makes each
    // add / swap / CAS atomic (no lost ticks, no torn reads) but
    // establishes no happens-before with other memory. Code that must
    // synchronise other state through the integer uses Shared\Atomic,
    // which exposes explicit Ordering. Do not strengthen this without
    // updating the public Counter stubs, docs, and CHANGELOG.
    pub fn get(&self) -> i64 {
        self.value.load(Ordering::Relaxed)
    }

    pub fn swap(&self, v: i64) -> i64 {
        self.value.swap(v, Ordering::Relaxed)
    }

    /// Returns the NEW value.
    pub fn add(&self, delta: i64) -> i64 {
        self.value
            .fetch_add(delta, Ordering::Relaxed)
            .wrapping_add(delta)
    }

    /// CAS. Returns true if the current value was `expect` and was
    /// replaced with `new`. Relaxed/Relaxed — sufficient for bounded /
    /// saturating counters whose decision is made on the counter's own
    /// value. Ordered CAS belongs to Shared\Atomic.
    pub fn compare_and_set(&self, expect: i64, new: i64) -> bool {
        self.value
            .compare_exchange(expect, new, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }
}

impl SharedInner for CounterInner {
    fn type_tag(&self) -> SharedType {
        SharedType::Counter
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn debug_snapshot(&self) -> SharedValue {
        SharedValue::Long(self.get())
    }
    fn mem_bytes(&self) -> usize {
        std::mem::size_of::<Self>()
    }
    fn on_drop(&self) {}
    fn on_shutdown_notify(&self) {}
}

// ─── FFI ──────────────────────────────────────────────────────────────

/// # Safety
/// `out_ptr` must be valid for writes of `*const Entry` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_counter_create(
    initial: i64,
    out_ptr: *mut *const Entry,
) -> c_int {
    if out_ptr.is_null() {
        set_last_error("out_ptr is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let inner = Arc::new(CounterInner::new(initial));
        let arc = reg.insert(SharedType::Counter, inner)?;
        // SAFETY: out_ptr checked non-null above.
        unsafe { *out_ptr = Arc::into_raw(arc) };
        Ok(())
    })
}

/// # Safety
/// `entry_ptr` must be a live `Arc::into_raw` pointer or NULL.
/// `out` must be valid for writes of `i64` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_counter_get(entry_ptr: *const Entry, out: *mut i64) -> c_int {
    if out.is_null() {
        set_last_error("out is null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        // SAFETY: entry_ptr non-null and, per the handle contract, a
        // live Arc::into_raw pointer — the calling PHP wrapper holds a
        // strong ref through it.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "counter_get on freed Entry");
        let inner = entry.inner.as_any_counter().ok_or(SharedError::Type)?;
        let v = inner.get();
        entry.registry.record_op(entry);
        // SAFETY: out checked non-null above.
        unsafe { *out = v };
        Ok(())
    })
}

/// # Safety
/// `entry_ptr` must be a live `Arc::into_raw` pointer or NULL.
/// `out_prev` must be valid for writes of `i64` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_counter_swap(
    entry_ptr: *const Entry,
    new_val: i64,
    out_prev: *mut i64,
) -> c_int {
    if out_prev.is_null() {
        set_last_error("out_prev is null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_counter_get.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "counter_swap on freed Entry");
        let inner = entry.inner.as_any_counter().ok_or(SharedError::Type)?;
        let prev = inner.swap(new_val);
        entry.registry.record_op(entry);
        // SAFETY: out_prev checked non-null above.
        unsafe { *out_prev = prev };
        Ok(())
    })
}

/// # Safety
/// `entry_ptr` must be a live `Arc::into_raw` pointer or NULL.
/// `out_new` must be valid for writes of `i64` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_counter_add(
    entry_ptr: *const Entry,
    delta: i64,
    out_new: *mut i64,
) -> c_int {
    if out_new.is_null() {
        set_last_error("out_new is null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        // SAFETY: see oxphp_shared_counter_get.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "counter_add on freed Entry");
        let inner = entry.inner.as_any_counter().ok_or(SharedError::Type)?;
        let new_val = inner.add(delta);
        entry.registry.record_op(entry);
        // SAFETY: out_new checked non-null above.
        unsafe { *out_new = new_val };
        Ok(())
    })
}

/// # Safety
/// `entry_ptr` must be a live `Arc::into_raw` pointer or NULL.
/// `out_swapped` must be valid for writes of `c_int` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_counter_cas(
    entry_ptr: *const Entry,
    expect: i64,
    new_val: i64,
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
        // SAFETY: see oxphp_shared_counter_get.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "counter_cas on freed Entry");
        let inner = entry.inner.as_any_counter().ok_or(SharedError::Type)?;
        let swapped = inner.compare_and_set(expect, new_val);
        entry.registry.record_op(entry);
        // SAFETY: out_swapped checked non-null above.
        unsafe { *out_swapped = swapped as c_int };
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
        self.as_any().downcast_ref::<CounterInner>()
    }
}

// ─── PHP class registration ───────────────────────────────────────────

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
            let mut out_ptr: *const Entry = std::ptr::null();
            let rc = unsafe { oxphp_shared_counter_create(initial, &mut out_ptr) };
            if rc != 0 {
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\SharedException".to_string(),
                    message: read_last_error_message(),
                    code: 0,
                });
            }
            let handle = call.storage_mut::<SharedHandle>()?;
            handle.entry_ptr = out_ptr;
            handle.type_tag = SharedType::Counter as u8;
            Ok(())
        })
        .method("get")
        .returns(PhpType::Int)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let mut out: i64 = 0;
            let rc = unsafe { oxphp_shared_counter_get(handle.entry_ptr, &mut out) };
            counter_rc_to_result(rc)?;
            call.ret_long(out);
            Ok(())
        })
        .method("set")
        .param("value", PhpType::Int)
        .returns(PhpType::Int)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let value = call.arg_long(0)?;
            let mut prev: i64 = 0;
            let rc = unsafe { oxphp_shared_counter_swap(handle.entry_ptr, value, &mut prev) };
            counter_rc_to_result(rc)?;
            call.ret_long(prev);
            Ok(())
        })
        .method("add")
        .optional_param("delta", PhpType::Int, PhpValue::Int(1))
        .returns(PhpType::Int)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let delta = if call.argc() > 0 {
                call.arg_long(0).unwrap_or(1)
            } else {
                1
            };
            let mut new_val: i64 = 0;
            let rc = unsafe { oxphp_shared_counter_add(handle.entry_ptr, delta, &mut new_val) };
            counter_rc_to_result(rc)?;
            call.ret_long(new_val);
            Ok(())
        })
        .method("compareAndSet")
        .param("expect", PhpType::Int)
        .param("new", PhpType::Int)
        .returns(PhpType::Bool)
        .handler(|call| {
            let handle = call.storage::<SharedHandle>()?;
            let expect = call.arg_long(0)?;
            let new_val = call.arg_long(1)?;
            let mut swapped: c_int = 0;
            let rc = unsafe {
                oxphp_shared_counter_cas(handle.entry_ptr, expect, new_val, &mut swapped)
            };
            counter_rc_to_result(rc)?;
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

pub(super) fn counter_rc_to_result(rc: c_int) -> Result<(), PhpError> {
    if rc == 0 {
        return Ok(());
    }
    let msg = read_last_error_message();
    let class = match rc {
        -2 => "OxPHP\\Shared\\StaleHandleException",
        -3 => "OxPHP\\Shared\\TypeException",
        -4 => "OxPHP\\Shared\\CapacityException",
        -9 => "OxPHP\\Shared\\CycleException",
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
    fn compare_and_set_swaps_only_on_match() {
        let c = CounterInner::new(5);
        assert!(c.compare_and_set(5, 10));
        assert_eq!(c.get(), 10);
        // current is 10, not 5 → no swap
        assert!(!c.compare_and_set(5, 20));
        assert_eq!(c.get(), 10);
    }

    #[test]
    fn overflow_wraps() {
        let c = CounterInner::new(i64::MAX);
        let new = c.add(1);
        assert_eq!(new, i64::MIN);
    }
}
