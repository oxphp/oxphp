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

// Helper trait for downcasting `Arc<dyn SharedInner>` to `&AtomicInner`.
// Implemented on `dyn SharedInner` (not `+ Send + Sync`) to match `Entry.inner`'s
// actual trait-object type.
//
// NOTE: this mirrors the pre-existing fat-pointer-cast pattern used by every
// other Shared\* type (`SharedInnerCounterExt`, `SharedInnerChannelExt`, etc.).
// The idiomatic alternative is to add `fn as_any(&self) -> &dyn Any` on the
// `SharedInner` trait and use `Any::downcast_ref`; that's a worthwhile cleanup
// across the whole module but out of scope for the Atomic introduction.
pub trait SharedInnerAtomicExt {
    fn as_any_atomic(&self) -> Option<&AtomicInner>;
}

impl SharedInnerAtomicExt for dyn SharedInner {
    fn as_any_atomic(&self) -> Option<&AtomicInner> {
        if self.type_tag() == SharedType::Atomic {
            // SAFETY: type_tag() == Atomic guarantees the concrete type is
            // AtomicInner. Casting a `*const dyn SharedInner` fat pointer to
            // `*const AtomicInner` yields the data pointer, which is the
            // address of the AtomicInner allocation. Sound as long as
            // SharedType::Atomic is only ever used with AtomicInner — enforced
            // by the sole insertion site in oxphp_shared_atomic_create.
            Some(unsafe { &*(self as *const dyn SharedInner as *const AtomicInner) })
        } else {
            None
        }
    }
}

// ─── FFI ──────────────────────────────────────────────────────────────

use std::os::raw::c_int;
use std::sync::Arc;

use crate::plugins::ox_shared::error::{
    ffi_entry, read_last_error_message, set_last_error, SharedError,
};
use crate::plugins::ox_shared::registry::registry;

fn ordering_from_u8(v: u8) -> Ordering {
    match v {
        0 => Ordering::Relaxed,
        1 => Ordering::Acquire,
        2 => Ordering::Release,
        3 => Ordering::AcqRel,
        4 => Ordering::SeqCst,
        // Out-of-range input means a caller bypassed `read_order_arg` (whose
        // range check is the primary defence). In debug builds we want to
        // catch that immediately; in release we fall back to SeqCst, which is
        // strictly stronger than any other valid ordering, so behaviour stays
        // safe even if a value slips through.
        _ => {
            debug_assert!(false, "ordering value {v} out of range; expected 0..=4");
            Ordering::SeqCst
        }
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

// ─── Memory-ordering validation ──────────────────────────────────────
//
// `std::sync::atomic` panics on bad ordering combinations. We catch them
// at the PHP boundary and raise a typed exception instead, so PHP users
// get a clear error rather than a "Rust panic" generic.
//
// Consumed by the PHP class handlers registered below.

use crate::plugin::php::PhpError;

const ORDERING_NAMES: [&str; 5] = ["Relaxed", "Acquire", "Release", "AcqRel", "SeqCst"];

fn ordering_name(v: u8) -> &'static str {
    ORDERING_NAMES.get(v as usize).copied().unwrap_or("?")
}

fn invalid_ordering(op: &str, v: u8, allowed: &str) -> PhpError {
    PhpError::Exception {
        class: "OxPHP\\Shared\\InvalidOrderingException".to_string(),
        message: format!(
            "{op}() cannot use Ordering::{name} — allowed: {allowed}",
            op = op,
            name = ordering_name(v),
            allowed = allowed,
        ),
        code: 0,
    }
}

pub(crate) fn validate_ordering_for_load(v: u8) -> Result<(), PhpError> {
    match v {
        0 | 1 | 4 => Ok(()), // Relaxed, Acquire, SeqCst
        _ => Err(invalid_ordering("load", v, "Relaxed, Acquire, SeqCst")),
    }
}

pub(crate) fn validate_ordering_for_store(v: u8) -> Result<(), PhpError> {
    match v {
        0 | 2 | 4 => Ok(()), // Relaxed, Release, SeqCst
        _ => Err(invalid_ordering("store", v, "Relaxed, Release, SeqCst")),
    }
}

pub(crate) fn validate_cas_failure_ordering(v: u8) -> Result<(), PhpError> {
    match v {
        0 | 1 | 4 => Ok(()),
        _ => Err(invalid_ordering(
            "compareAndSet failure",
            v,
            "Relaxed, Acquire, SeqCst",
        )),
    }
}

// ─── PHP class registration ───────────────────────────────────────────

use crate::plugin::types::{MagicMethod, PhpType, PhpValue};
use crate::plugin::{PluginContext, PluginError};
use crate::plugins::ox_shared::handle::SharedHandle;

fn atomic_rc_to_result(rc: c_int) -> Result<(), PhpError> {
    if rc == 0 {
        return Ok(());
    }
    let class = match rc {
        -2 => "OxPHP\\Shared\\StaleHandleException",
        -3 => "OxPHP\\Shared\\TypeException",
        -4 => "OxPHP\\Shared\\CapacityException",
        -10 => "OxPHP\\Shared\\UninitializedException",
        _ => "OxPHP\\Shared\\SharedException",
    };
    Err(PhpError::Exception {
        class: class.to_string(),
        message: read_last_error_message(),
        code: 0,
    })
}

/// Read the optional `order` enum argument at `idx`. Returns the SeqCst
/// default (4) if the caller omitted it. Range-checks the backed value
/// before returning — `Shared\Ordering` always backs to 0..=4, but a
/// caller who reaches us through reflection or a bug could plant garbage,
/// and silently mapping it to SeqCst would mask the problem.
fn read_order_arg(call: &mut crate::bridge::call::NativeCall, idx: u32) -> Result<u8, PhpError> {
    if call.argc() > idx {
        let v = call.arg_enum_long(idx)?;
        if !(0..=4).contains(&v) {
            return Err(PhpError::Exception {
                class: "OxPHP\\Shared\\InvalidOrderingException".to_string(),
                message: format!(
                    "Ordering value {v} out of range \
                     — expected 0..=4 (Relaxed, Acquire, Release, AcqRel, SeqCst)"
                ),
                code: 0,
            });
        }
        Ok(v as u8)
    } else {
        Ok(4)
    }
}

fn order_default() -> PhpValue {
    PhpValue::ConstExpr("\\OxPHP\\Shared\\Ordering::SeqCst".to_string())
}

fn order_type() -> PhpType {
    PhpType::Enum("OxPHP\\Shared\\Ordering".to_string())
}

pub fn register_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_class("OxPHP\\Shared\\Atomic")
        .implements("OxPHP\\Shared\\Shareable")
        .with_storage(|| SharedHandle::new(SharedType::Atomic))
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
            let rc = unsafe { oxphp_shared_atomic_create(initial, &mut out_id) };
            if rc != 0 {
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\SharedException".to_string(),
                    message: read_last_error_message(),
                    code: 0,
                });
            }
            let handle = call.storage_mut::<SharedHandle>()?;
            handle.shared_id = out_id;
            handle.type_tag = SharedType::Atomic as u8;
            Ok(())
        })
        .method("load")
        .optional_param("order", order_type(), order_default())
        .returns(PhpType::Int)
        .handler(|call| {
            let order = read_order_arg(call, 0)?;
            validate_ordering_for_load(order)?;
            let handle = call.storage::<SharedHandle>()?;
            let mut out: i64 = 0;
            let rc = unsafe { oxphp_shared_atomic_load(handle.shared_id, order, &mut out) };
            atomic_rc_to_result(rc)?;
            call.ret_long(out);
            Ok(())
        })
        .method("store")
        .param("value", PhpType::Int)
        .optional_param("order", order_type(), order_default())
        .returns(PhpType::Void)
        .handler(|call| {
            let value = call.arg_long(0)?;
            let order = read_order_arg(call, 1)?;
            validate_ordering_for_store(order)?;
            let handle = call.storage::<SharedHandle>()?;
            let rc = unsafe { oxphp_shared_atomic_store(handle.shared_id, value, order) };
            atomic_rc_to_result(rc)?;
            Ok(())
        })
        .method("swap")
        .param("value", PhpType::Int)
        .optional_param("order", order_type(), order_default())
        .returns(PhpType::Int)
        .handler(|call| {
            let value = call.arg_long(0)?;
            let order = read_order_arg(call, 1)?;
            let handle = call.storage::<SharedHandle>()?;
            let mut prev: i64 = 0;
            let rc = unsafe { oxphp_shared_atomic_swap(handle.shared_id, value, order, &mut prev) };
            atomic_rc_to_result(rc)?;
            call.ret_long(prev);
            Ok(())
        })
        .method("compareAndSet")
        .param("expect", PhpType::Int)
        .param("new", PhpType::Int)
        .optional_param("success", order_type(), order_default())
        .optional_param("failure", order_type(), order_default())
        .returns(PhpType::Bool)
        .handler(|call| {
            let expect = call.arg_long(0)?;
            let new_val = call.arg_long(1)?;
            let success = read_order_arg(call, 2)?;
            let failure = read_order_arg(call, 3)?;
            validate_cas_failure_ordering(failure)?;
            let handle = call.storage::<SharedHandle>()?;
            let mut swapped: c_int = 0;
            let rc = unsafe {
                oxphp_shared_atomic_cas(
                    handle.shared_id,
                    expect,
                    new_val,
                    success,
                    failure,
                    &mut swapped,
                )
            };
            atomic_rc_to_result(rc)?;
            call.ret_bool(swapped != 0);
            Ok(())
        })
        .method("fetchAdd")
        .param("delta", PhpType::Int)
        .optional_param("order", order_type(), order_default())
        .returns(PhpType::Int)
        .handler(|call| {
            let delta = call.arg_long(0)?;
            let order = read_order_arg(call, 1)?;
            let handle = call.storage::<SharedHandle>()?;
            let mut prev: i64 = 0;
            let rc =
                unsafe { oxphp_shared_atomic_fetch_add(handle.shared_id, delta, order, &mut prev) };
            atomic_rc_to_result(rc)?;
            call.ret_long(prev);
            Ok(())
        })
        .method("fetchSub")
        .param("delta", PhpType::Int)
        .optional_param("order", order_type(), order_default())
        .returns(PhpType::Int)
        .handler(|call| {
            let delta = call.arg_long(0)?;
            let order = read_order_arg(call, 1)?;
            let handle = call.storage::<SharedHandle>()?;
            let mut prev: i64 = 0;
            let rc =
                unsafe { oxphp_shared_atomic_fetch_sub(handle.shared_id, delta, order, &mut prev) };
            atomic_rc_to_result(rc)?;
            call.ret_long(prev);
            Ok(())
        })
        .method("fetchAnd")
        .param("mask", PhpType::Int)
        .optional_param("order", order_type(), order_default())
        .returns(PhpType::Int)
        .handler(|call| {
            let mask = call.arg_long(0)?;
            let order = read_order_arg(call, 1)?;
            let handle = call.storage::<SharedHandle>()?;
            let mut prev: i64 = 0;
            let rc =
                unsafe { oxphp_shared_atomic_fetch_and(handle.shared_id, mask, order, &mut prev) };
            atomic_rc_to_result(rc)?;
            call.ret_long(prev);
            Ok(())
        })
        .method("fetchOr")
        .param("mask", PhpType::Int)
        .optional_param("order", order_type(), order_default())
        .returns(PhpType::Int)
        .handler(|call| {
            let mask = call.arg_long(0)?;
            let order = read_order_arg(call, 1)?;
            let handle = call.storage::<SharedHandle>()?;
            let mut prev: i64 = 0;
            let rc =
                unsafe { oxphp_shared_atomic_fetch_or(handle.shared_id, mask, order, &mut prev) };
            atomic_rc_to_result(rc)?;
            call.ret_long(prev);
            Ok(())
        })
        .method("fetchXor")
        .param("mask", PhpType::Int)
        .optional_param("order", order_type(), order_default())
        .returns(PhpType::Int)
        .handler(|call| {
            let mask = call.arg_long(0)?;
            let order = read_order_arg(call, 1)?;
            let handle = call.storage::<SharedHandle>()?;
            let mut prev: i64 = 0;
            let rc =
                unsafe { oxphp_shared_atomic_fetch_xor(handle.shared_id, mask, order, &mut prev) };
            atomic_rc_to_result(rc)?;
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
    fn validate_ordering_for_load_rejects_release_acqrel() {
        assert!(validate_ordering_for_load(0).is_ok()); // Relaxed
        assert!(validate_ordering_for_load(1).is_ok()); // Acquire
        assert!(validate_ordering_for_load(4).is_ok()); // SeqCst
        assert!(validate_ordering_for_load(2).is_err()); // Release
        assert!(validate_ordering_for_load(3).is_err()); // AcqRel
    }

    #[test]
    fn validate_ordering_for_store_rejects_acquire_acqrel() {
        assert!(validate_ordering_for_store(0).is_ok()); // Relaxed
        assert!(validate_ordering_for_store(2).is_ok()); // Release
        assert!(validate_ordering_for_store(4).is_ok()); // SeqCst
        assert!(validate_ordering_for_store(1).is_err()); // Acquire
        assert!(validate_ordering_for_store(3).is_err()); // AcqRel
    }

    #[test]
    fn ordering_from_u8_in_range_round_trips() {
        assert_eq!(ordering_from_u8(0), Ordering::Relaxed);
        assert_eq!(ordering_from_u8(1), Ordering::Acquire);
        assert_eq!(ordering_from_u8(2), Ordering::Release);
        assert_eq!(ordering_from_u8(3), Ordering::AcqRel);
        assert_eq!(ordering_from_u8(4), Ordering::SeqCst);
    }

    #[test]
    fn validate_cas_failure_rejects_release_acqrel() {
        assert!(validate_cas_failure_ordering(0).is_ok());
        assert!(validate_cas_failure_ordering(1).is_ok());
        assert!(validate_cas_failure_ordering(4).is_ok());
        assert!(validate_cas_failure_ordering(2).is_err());
        assert!(validate_cas_failure_ordering(3).is_err());
    }

    #[test]
    fn ffi_create_load_store_round_trip() {
        ensure_registry();

        let mut id: u64 = 0;
        let rc = unsafe { oxphp_shared_atomic_create(100, &mut id) };
        assert_eq!(rc, 0);
        assert!(id > 0);

        let mut out: i64 = 0;
        let rc = unsafe {
            oxphp_shared_atomic_load(id, 4 /* SeqCst */, &mut out)
        };
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
