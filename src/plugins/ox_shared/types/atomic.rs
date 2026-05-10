//! Shared\Atomic — generic atomic int64 primitive (load, store, swap, CAS,
//! fetch-arithmetic, fetch-bitwise) with explicit memory ordering control.

use std::sync::atomic::{AtomicI64, Ordering};

use crate::plugins::ox_shared::registry::{SharedInner, SharedType};
use crate::plugins::ox_shared::value::SharedValue;

pub struct AtomicInner {
    value: AtomicI64,
}

impl AtomicInner {
    pub fn new(initial: i64) -> Self {
        Self {
            value: AtomicI64::new(initial),
        }
    }

    pub fn load(&self, order: Ordering) -> i64 {
        self.value.load(order)
    }

    pub fn store(&self, v: i64, order: Ordering) {
        self.value.store(v, order);
    }

    pub fn swap(&self, v: i64, order: Ordering) -> i64 {
        self.value.swap(v, order)
    }

    pub fn compare_and_set(
        &self,
        expect: i64,
        new: i64,
        success: Ordering,
        failure: Ordering,
    ) -> bool {
        self.value
            .compare_exchange(expect, new, success, failure)
            .is_ok()
    }

    pub fn fetch_add(&self, delta: i64, order: Ordering) -> i64 {
        self.value.fetch_add(delta, order)
    }

    pub fn fetch_sub(&self, delta: i64, order: Ordering) -> i64 {
        self.value.fetch_sub(delta, order)
    }

    pub fn fetch_and(&self, mask: i64, order: Ordering) -> i64 {
        self.value.fetch_and(mask, order)
    }

    pub fn fetch_or(&self, mask: i64, order: Ordering) -> i64 {
        self.value.fetch_or(mask, order)
    }

    pub fn fetch_xor(&self, mask: i64, order: Ordering) -> i64 {
        self.value.fetch_xor(mask, order)
    }
}

impl SharedInner for AtomicInner {
    fn type_tag(&self) -> SharedType {
        SharedType::Atomic
    }
    fn debug_snapshot(&self) -> SharedValue {
        SharedValue::Long(self.load(Ordering::SeqCst))
    }
    fn mem_bytes(&self) -> usize {
        16
    }
    fn on_drop(&self) {}
    fn on_shutdown_notify(&self) {}
}

// Downcast `Arc<dyn SharedInner>` to `&AtomicInner`. Sound because the only
// insertion site for `SharedType::Atomic` is `oxphp_shared_atomic_create`,
// which inserts an `Arc<AtomicInner>`.
pub trait SharedInnerAtomicExt {
    fn as_any_atomic(&self) -> Option<&AtomicInner>;
}

impl SharedInnerAtomicExt for dyn SharedInner {
    fn as_any_atomic(&self) -> Option<&AtomicInner> {
        if self.type_tag() == SharedType::Atomic {
            Some(unsafe { &*(self as *const dyn SharedInner as *const AtomicInner) })
        } else {
            None
        }
    }
}

// ─── FFI ──────────────────────────────────────────────────────────────

use std::os::raw::c_int;
use std::sync::Arc;

use crate::plugins::ox_shared::error::{ffi_entry, set_last_error, SharedError};
use crate::plugins::ox_shared::registry::registry;

fn ordering_from_u8(v: u8) -> Ordering {
    match v {
        0 => Ordering::Relaxed,
        1 => Ordering::Acquire,
        2 => Ordering::Release,
        3 => Ordering::AcqRel,
        _ => Ordering::SeqCst,
    }
}

/// # Safety
/// `out_id` must be valid for writes of `u64` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_atomic_create(initial: i64, out_id: *mut u64) -> c_int {
    if out_id.is_null() {
        set_last_error("out_id is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let inner = Arc::new(AtomicInner::new(initial));
        let id = reg.insert(SharedType::Atomic, inner)?;
        unsafe { *out_id = id };
        Ok(())
    })
}

/// # Safety
/// `out` must be valid for writes of `i64` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_atomic_load(id: u64, order: u8, out: *mut i64) -> c_int {
    if out.is_null() {
        set_last_error("out is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_atomic().ok_or(SharedError::Type)?;
        let v = inner.load(ordering_from_u8(order));
        reg.record_op(id);
        unsafe { *out = v };
        Ok(())
    })
}

/// # Safety
/// `id` must reference a live registry entry.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_atomic_store(id: u64, value: i64, order: u8) -> c_int {
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_atomic().ok_or(SharedError::Type)?;
        inner.store(value, ordering_from_u8(order));
        reg.record_op(id);
        Ok(())
    })
}

/// # Safety
/// `out_prev` must be valid for writes of `i64` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_atomic_swap(
    id: u64,
    value: i64,
    order: u8,
    out_prev: *mut i64,
) -> c_int {
    if out_prev.is_null() {
        set_last_error("out_prev is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_atomic().ok_or(SharedError::Type)?;
        let prev = inner.swap(value, ordering_from_u8(order));
        reg.record_op(id);
        unsafe { *out_prev = prev };
        Ok(())
    })
}

/// # Safety
/// `out_swapped` must be valid for writes of `c_int` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_atomic_cas(
    id: u64,
    expect: i64,
    new_val: i64,
    success: u8,
    failure: u8,
    out_swapped: *mut c_int,
) -> c_int {
    if out_swapped.is_null() {
        set_last_error("out_swapped is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_atomic().ok_or(SharedError::Type)?;
        let swapped = inner.compare_and_set(
            expect,
            new_val,
            ordering_from_u8(success),
            ordering_from_u8(failure),
        );
        reg.record_op(id);
        unsafe { *out_swapped = swapped as c_int };
        Ok(())
    })
}

macro_rules! atomic_fetch_ffi {
    ($fn_name:ident, $method:ident) => {
        /// # Safety
        /// `out_prev` must be valid for writes of `i64` if non-null.
        #[no_mangle]
        pub unsafe extern "C" fn $fn_name(
            id: u64,
            delta: i64,
            order: u8,
            out_prev: *mut i64,
        ) -> c_int {
            if out_prev.is_null() {
                set_last_error("out_prev is null");
                return SharedError::Generic.code();
            }
            ffi_entry(|| {
                let reg = registry();
                let entry = reg.lookup(id)?;
                let inner = entry.inner.as_any_atomic().ok_or(SharedError::Type)?;
                let prev = inner.$method(delta, ordering_from_u8(order));
                reg.record_op(id);
                unsafe { *out_prev = prev };
                Ok(())
            })
        }
    };
}

atomic_fetch_ffi!(oxphp_shared_atomic_fetch_add, fetch_add);
atomic_fetch_ffi!(oxphp_shared_atomic_fetch_sub, fetch_sub);
atomic_fetch_ffi!(oxphp_shared_atomic_fetch_and, fetch_and);
atomic_fetch_ffi!(oxphp_shared_atomic_fetch_or, fetch_or);
atomic_fetch_ffi!(oxphp_shared_atomic_fetch_xor, fetch_xor);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_store_swap_baseline() {
        let a = AtomicInner::new(0);
        a.store(42, Ordering::SeqCst);
        assert_eq!(a.load(Ordering::SeqCst), 42);
        assert_eq!(a.swap(7, Ordering::SeqCst), 42);
        assert_eq!(a.load(Ordering::Acquire), 7);
    }

    #[test]
    fn cas_success_and_failure_paths() {
        let a = AtomicInner::new(10);
        assert!(a.compare_and_set(10, 20, Ordering::SeqCst, Ordering::SeqCst));
        assert!(!a.compare_and_set(10, 30, Ordering::SeqCst, Ordering::Acquire));
        assert_eq!(a.load(Ordering::SeqCst), 20);
    }

    #[test]
    fn fetch_add_returns_prev() {
        let a = AtomicInner::new(5);
        assert_eq!(a.fetch_add(3, Ordering::SeqCst), 5);
        assert_eq!(a.load(Ordering::SeqCst), 8);
    }

    #[test]
    fn fetch_sub_overflow_wraps() {
        let a = AtomicInner::new(i64::MIN);
        let prev = a.fetch_sub(1, Ordering::SeqCst);
        assert_eq!(prev, i64::MIN);
        assert_eq!(a.load(Ordering::SeqCst), i64::MAX);
    }

    #[test]
    fn fetch_bitwise_known_masks() {
        let a = AtomicInner::new(0b1010);
        assert_eq!(a.fetch_and(0b1100, Ordering::SeqCst), 0b1010);
        assert_eq!(a.load(Ordering::SeqCst), 0b1000);
        assert_eq!(a.fetch_or(0b0011, Ordering::SeqCst), 0b1000);
        assert_eq!(a.load(Ordering::SeqCst), 0b1011);
        assert_eq!(a.fetch_xor(0b1111, Ordering::SeqCst), 0b1011);
        assert_eq!(a.load(Ordering::SeqCst), 0b0100);
    }

    use crate::plugins::ox_shared::config::{LockDiagnosticsLevel, SharedConfig};
    use crate::plugins::ox_shared::registry::init_registry;

    fn ensure_registry() {
        // Idempotent — OnceLock.set drops the dupe silently. Concurrent tests
        // that hit the registry call this; the first one wins.
        init_registry(SharedConfig {
            enabled: true,
            max_entries: 10_000,
            max_bytes: 1 << 30,
            soft_limit_ratio: 0.7,
            metrics_enabled: false,
            introspection_enabled: false,
            introspection_preview_enabled: false,
            cycle_detect_depth: 16,
            cycle_detect_edges: 10_000,
            shutdown_timeout_seconds: 5.0,
            poison_strict: false,
            lock_diagnostics: LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
            preview_string_limit: 256,
            preview_array_limit: 20,
        });
    }

    #[test]
    fn ffi_create_load_store_round_trip() {
        ensure_registry();

        let mut id: u64 = 0;
        let rc = unsafe { oxphp_shared_atomic_create(100, &mut id) };
        assert_eq!(rc, 0);
        assert!(id > 0);

        let mut out: i64 = 0;
        let rc = unsafe { oxphp_shared_atomic_load(id, 4 /* SeqCst */, &mut out) };
        assert_eq!(rc, 0);
        assert_eq!(out, 100);

        let rc = unsafe { oxphp_shared_atomic_store(id, 7, 4) };
        assert_eq!(rc, 0);

        let mut prev: i64 = 0;
        let rc = unsafe { oxphp_shared_atomic_swap(id, 99, 4, &mut prev) };
        assert_eq!(rc, 0);
        assert_eq!(prev, 7);

        let mut swapped: c_int = 0;
        let rc = unsafe { oxphp_shared_atomic_cas(id, 99, 200, 4, 4, &mut swapped) };
        assert_eq!(rc, 0);
        assert_eq!(swapped, 1);

        let mut prev_add: i64 = 0;
        let rc = unsafe { oxphp_shared_atomic_fetch_add(id, 5, 4, &mut prev_add) };
        assert_eq!(rc, 0);
        assert_eq!(prev_add, 200);
    }
}
