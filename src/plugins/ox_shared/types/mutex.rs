//! Shared\Mutex — exclusive lock over a SharedValue.

use std::os::raw::c_int;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::plugins::ox_shared::types::timeout::{parse_timeout, read_positive_ms_arg, Wait};

use parking_lot::Mutex;

use crate::bridge::types::ValType;
use crate::plugin::types::{MagicMethod, PhpType, PhpValue};
use crate::plugin::{PhpError, PluginContext, PluginError};
use crate::plugins::ox_shared::error::{
    ffi_entry, read_last_error_message, set_last_error, SharedError,
};
use crate::plugins::ox_shared::handle::SharedHandle;
use crate::plugins::ox_shared::registry::{registry, Entry, SharedInner, SharedType, ENTRY_MAGIC};
use crate::plugins::ox_shared::value::{portbuf_to_sv, raw_to_owned, sv_to_portbuf, SharedValue};

/// Per-mutex storage. `state` is parking_lot::Mutex so try_lock_for
/// is available. `corrupted` is split out for lock-free is_corrupted();
/// it is sticky — once set, there is no public API to clear it.
pub struct MutexInner {
    pub(crate) state: Mutex<SharedValue>,
    corrupted: AtomicBool,
}

impl MutexInner {
    pub fn new(initial: SharedValue) -> Self {
        Self {
            state: Mutex::new(initial),
            corrupted: AtomicBool::new(false),
        }
    }

    pub fn is_corrupted(&self) -> bool {
        self.corrupted.load(Ordering::Acquire)
    }

    pub fn mark_corrupted(&self) {
        self.corrupted.store(true, Ordering::Release);
    }

    /// Snapshot the current state — non-blocking; returns Null if contended.
    pub fn try_snapshot(&self) -> SharedValue {
        match self.state.try_lock() {
            Some(g) => g.clone(),
            None => SharedValue::Null,
        }
    }
}

impl SharedInner for MutexInner {
    fn type_tag(&self) -> SharedType {
        SharedType::Mutex
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn debug_snapshot(&self) -> SharedValue {
        self.try_snapshot()
    }
    fn mem_bytes(&self) -> usize {
        32 + self.try_snapshot().mem_bytes()
    }
    fn on_drop(&self) {}
    fn on_shutdown_notify(&self) {}
}

pub trait SharedInnerMutexExt {
    fn as_any_mutex(&self) -> Option<&MutexInner>;
}

impl SharedInnerMutexExt for dyn SharedInner {
    fn as_any_mutex(&self) -> Option<&MutexInner> {
        self.as_any().downcast_ref::<MutexInner>()
    }
}

// ─── FFI ──────────────────────────────────────────────────────────────

/// # Safety
/// `initial_buf` must be valid for reads of `initial_len` bytes encoding
/// a SharedValue in portbuf format. `out_ptr` must be valid for writes of
/// `*const Entry` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_mutex_create(
    initial_buf: *const u8,
    initial_len: usize,
    out_ptr: *mut *const Entry,
) -> c_int {
    if out_ptr.is_null() {
        set_last_error("out_ptr is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let initial = if initial_buf.is_null() || initial_len == 0 {
            SharedValue::Null
        } else {
            let bytes = unsafe { std::slice::from_raw_parts(initial_buf, initial_len) };
            let raw = portbuf_to_sv(bytes)?;
            raw_to_owned(raw, reg)?
        };
        let arc = reg.insert(SharedType::Mutex, Arc::new(MutexInner::new(initial)))?;
        // SAFETY: out_ptr checked non-null above.
        unsafe { *out_ptr = Arc::into_raw(arc) };
        Ok(())
    })
}

use crate::plugins::ox_shared::reentrancy::{push_held, MutexPopGuard};

/// Invoke the closure under the mutex with bounded wait. The closure
/// receives the current state as an IS_REFERENCE zval materialised
/// from portbuf bytes; mutations persist if the closure returns
/// normally.
///
/// `timeout_ms` follows the wire convention: `-1` = forever, `0` = try,
/// `>0` = milliseconds. See `timeout::parse_timeout`.
///
/// # Safety
/// `entry_ptr` must be a live `Arc::into_raw` pointer or NULL.
/// `callable` must be a valid PHP zval*. `out_ret_buf`/`out_ret_len`
/// must be valid for writes.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_mutex_with(
    entry_ptr: *const Entry,
    callable: *mut std::ffi::c_void,
    timeout_ms: i64,
    out_ret_buf: *mut *mut u8,
    out_ret_len: *mut usize,
) -> c_int {
    if out_ret_buf.is_null() || out_ret_len.is_null() {
        set_last_error("out pointers null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    unsafe {
        *out_ret_buf = std::ptr::null_mut();
        *out_ret_len = 0;
    }

    ffi_entry(|| {
        use crate::bridge::ffi;

        // SAFETY: entry_ptr non-null and, per the handle contract, a
        // live Arc::into_raw pointer.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(
            entry.magic, ENTRY_MAGIC,
            "oxphp_shared_mutex_with on freed Entry"
        );
        let id = entry.id;
        let inner = entry.inner.as_any_mutex().ok_or(SharedError::Type)?;

        if inner.is_corrupted() {
            set_last_error("mutex corrupted by prior Rust panic");
            return Err(SharedError::Panicked);
        }

        // Reentrancy check BEFORE acquire; RAII pop on every exit.
        push_held(id)?;
        let _pop = MutexPopGuard(id);

        // Register with cross-thread wait-for graph BEFORE trying to
        // acquire. The guard auto-drops on all exit paths.
        let waiter = crate::plugins::ox_shared::deadlock::register_waiter(id);

        // Detector may have already signalled us to break a cycle.
        if crate::plugins::ox_shared::deadlock::consume_break_signal().is_some() {
            set_last_error("Mutex::with: detected wait-for cycle; breaking");
            drop(waiter);
            return Err(SharedError::Deadlock);
        }

        let acquired = match parse_timeout(timeout_ms) {
            Wait::Forever => {
                // Block until acquired, corrupted, or cycle-broken. Poll with a
                // 100ms quantum so corruption + cycle-break signals can
                // progress. The `waiter` guard is held alive across
                // iterations — its Drop only fires on early-return error paths
                // or when promoted via `promote_to_holder` after acquisition.
                loop {
                    if let Some(g) = inner.state.try_lock_for(Duration::from_millis(100)) {
                        break Some(g);
                    }
                    if inner.corrupted.load(Ordering::Acquire) {
                        set_last_error("Mutex::with: corrupted during wait");
                        drop(waiter);
                        return Err(SharedError::Panicked);
                    }
                    // Mirror the bounded path's None-branch cycle detection: a
                    // break signal targeted at this thread must abort the wait.
                    if crate::plugins::ox_shared::deadlock::consume_break_signal().is_some() {
                        set_last_error("Mutex::with: wait-for cycle detected during forever wait");
                        drop(waiter);
                        return Err(SharedError::Deadlock);
                    }
                }
            }
            Wait::Try => inner.state.try_lock(),
            Wait::Bounded(d) => inner.state.try_lock_for(d),
        };

        let (mut guard, _holder_guard) = match acquired {
            Some(g) => {
                let hg = waiter.promote_to_holder();
                (g, hg)
            }
            None => {
                drop(waiter);
                if crate::plugins::ox_shared::deadlock::consume_break_signal().is_some() {
                    set_last_error("Mutex::with: wait-for cycle detected during timeout");
                    return Err(SharedError::Deadlock);
                }
                set_last_error(format!("mutex acquire timed out (timeout_ms={timeout_ms})"));
                return Err(SharedError::Timeout);
            }
        };

        if inner.is_corrupted() {
            return Err(SharedError::Panicked);
        }

        let state_bytes = sv_to_portbuf(&guard);

        let mut new_state_buf: *mut u8 = std::ptr::null_mut();
        let mut new_state_len: usize = 0;
        let mut ret_buf: *mut u8 = std::ptr::null_mut();
        let mut ret_len: usize = 0;
        let mut did_mutate: c_int = 0;

        // Wrap the engine call in catch_unwind — a Rust panic during
        // invocation must mark the mutex corrupted (sticky) before
        // propagating.
        let invoke_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            ffi::oxphp_shared_invoke_byref_1_portbuf(
                callable,
                state_bytes.as_ptr(),
                state_bytes.len(),
                &mut new_state_buf,
                &mut new_state_len,
                &mut ret_buf,
                &mut ret_len,
                &mut did_mutate,
            )
        }));

        match invoke_result {
            Err(_panic) => {
                inner.mark_corrupted();
                if !new_state_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(new_state_buf) };
                }
                if !ret_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(ret_buf) };
                }
                set_last_error("Rust panic inside Mutex::with closure invocation");
                Err(SharedError::Panicked)
            }
            Ok(rc) if rc == ffi::OXPHP_SHARED_INVOKE_OK => {
                if did_mutate != 0 && !new_state_buf.is_null() && new_state_len > 0 {
                    let new_bytes =
                        unsafe { std::slice::from_raw_parts(new_state_buf, new_state_len) };
                    match portbuf_to_sv(new_bytes).and_then(|raw| raw_to_owned(raw, entry.registry))
                    {
                        Ok(new_sv) => {
                            *guard = new_sv;
                        }
                        Err(e) => {
                            unsafe { ffi::oxphp_portable_free(new_state_buf) };
                            if !ret_buf.is_null() {
                                unsafe { ffi::oxphp_portable_free(ret_buf) };
                            }
                            return Err(e);
                        }
                    }
                }
                if !new_state_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(new_state_buf) };
                }
                unsafe {
                    *out_ret_buf = ret_buf;
                    *out_ret_len = ret_len;
                }
                entry.registry.record_op(entry);
                Ok(())
            }
            Ok(rc) if rc == ffi::OXPHP_SHARED_INVOKE_PHP_THREW => {
                // Closure threw. Per spec: state NOT rolled back; EG(exception)
                // is set. Default policy: do NOT poison.
                // The C shim serialises partial state before returning
                // PHP_THREW, so apply it here to honour the no-rollback policy.
                if !new_state_buf.is_null() && new_state_len > 0 {
                    let new_bytes =
                        unsafe { std::slice::from_raw_parts(new_state_buf, new_state_len) };
                    if let Ok(new_sv) =
                        portbuf_to_sv(new_bytes).and_then(|raw| raw_to_owned(raw, entry.registry))
                    {
                        *guard = new_sv;
                    }
                    unsafe { ffi::oxphp_portable_free(new_state_buf) };
                }
                if !ret_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(ret_buf) };
                }
                entry.registry.record_op(entry);
                set_last_error("closure threw");
                Err(SharedError::Generic)
            }
            Ok(_) => {
                if !new_state_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(new_state_buf) };
                }
                if !ret_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(ret_buf) };
                }
                set_last_error("Mutex::with: arg is not a valid callable");
                Err(SharedError::Type)
            }
        }
    })
}

/// Non-blocking try-acquire. Status: 0 → success, -7 → contended,
/// -99 → corrupted, -3 → not a Mutex, -2 → stale.
///
/// # Safety
/// See `oxphp_shared_mutex_with`.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_mutex_try_with(
    entry_ptr: *const Entry,
    callable: *mut std::ffi::c_void,
    out_ret_buf: *mut *mut u8,
    out_ret_len: *mut usize,
) -> c_int {
    if out_ret_buf.is_null() || out_ret_len.is_null() {
        set_last_error("out pointers null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    unsafe {
        *out_ret_buf = std::ptr::null_mut();
        *out_ret_len = 0;
    }

    ffi_entry(|| {
        use crate::bridge::ffi;

        // SAFETY: entry_ptr non-null and, per the handle contract, a
        // live Arc::into_raw pointer.
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(
            entry.magic, ENTRY_MAGIC,
            "oxphp_shared_mutex_try_with on freed Entry"
        );
        let id = entry.id;
        let inner = entry.inner.as_any_mutex().ok_or(SharedError::Type)?;

        if inner.is_corrupted() {
            set_last_error("mutex corrupted by prior Rust panic");
            return Err(SharedError::Panicked);
        }

        push_held(id)?;
        let _pop = MutexPopGuard(id);

        let waiter = crate::plugins::ox_shared::deadlock::register_waiter(id);

        let (mut guard, _holder_guard) = match inner.state.try_lock() {
            Some(g) => {
                let hg = waiter.promote_to_holder();
                (g, hg)
            }
            None => {
                drop(waiter);
                set_last_error("mutex contended; tryWithLock returning ContentionException");
                return Err(SharedError::Timeout);
            }
        };

        if inner.is_corrupted() {
            set_last_error("mutex corrupted by prior Rust panic");
            return Err(SharedError::Panicked);
        }

        let state_bytes = sv_to_portbuf(&guard);

        let mut new_state_buf: *mut u8 = std::ptr::null_mut();
        let mut new_state_len: usize = 0;
        let mut ret_buf: *mut u8 = std::ptr::null_mut();
        let mut ret_len: usize = 0;
        let mut did_mutate: c_int = 0;

        let invoke_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| unsafe {
            ffi::oxphp_shared_invoke_byref_1_portbuf(
                callable,
                state_bytes.as_ptr(),
                state_bytes.len(),
                &mut new_state_buf,
                &mut new_state_len,
                &mut ret_buf,
                &mut ret_len,
                &mut did_mutate,
            )
        }));

        match invoke_result {
            Err(_) => {
                inner.mark_corrupted();
                if !new_state_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(new_state_buf) };
                }
                if !ret_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(ret_buf) };
                }
                set_last_error("Rust panic inside Mutex::tryWithLock closure invocation");
                Err(SharedError::Panicked)
            }
            Ok(rc) if rc == ffi::OXPHP_SHARED_INVOKE_OK => {
                if did_mutate != 0 && !new_state_buf.is_null() && new_state_len > 0 {
                    let new_bytes =
                        unsafe { std::slice::from_raw_parts(new_state_buf, new_state_len) };
                    if let Ok(new_sv) =
                        portbuf_to_sv(new_bytes).and_then(|raw| raw_to_owned(raw, entry.registry))
                    {
                        *guard = new_sv;
                    }
                }
                if !new_state_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(new_state_buf) };
                }
                unsafe {
                    *out_ret_buf = ret_buf;
                    *out_ret_len = ret_len;
                }
                entry.registry.record_op(entry);
                Ok(())
            }
            Ok(rc) if rc == ffi::OXPHP_SHARED_INVOKE_PHP_THREW => {
                // Mirror oxphp_shared_mutex_with's PHP_THREW branch: closure
                // mutated state by-ref before throwing, partial mutation is
                // documented as "acceptable; caller responsible for invariant
                // restoration". The C shim already serialised the post-throw
                // state into new_state_buf — apply it here so tryWithLock
                // and withLock have symmetric mutation visibility.
                if !new_state_buf.is_null() && new_state_len > 0 {
                    let new_bytes =
                        unsafe { std::slice::from_raw_parts(new_state_buf, new_state_len) };
                    if let Ok(new_sv) =
                        portbuf_to_sv(new_bytes).and_then(|raw| raw_to_owned(raw, entry.registry))
                    {
                        *guard = new_sv;
                    }
                }
                if !new_state_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(new_state_buf) };
                }
                if !ret_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(ret_buf) };
                }
                entry.registry.record_op(entry);
                set_last_error("closure threw");
                Err(SharedError::Generic)
            }
            Ok(_) => {
                if !new_state_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(new_state_buf) };
                }
                if !ret_buf.is_null() {
                    unsafe { ffi::oxphp_portable_free(ret_buf) };
                }
                set_last_error("Mutex::tryWithLock: arg is not a valid callable");
                Err(SharedError::Type)
            }
        }
    })
}

// ─── Class registration ───────────────────────────────────────────────

pub fn register_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_class("OxPHP\\Shared\\Mutex")
        .implements("OxPHP\\Shared\\Shareable")
        .with_storage(|| SharedHandle::new(SharedType::Mutex))
        .magic(MagicMethod::Clone)
        .handler(|_call| {
            Err(PhpError::Exception {
                class: "OxPHP\\Shared\\SharedException".into(),
                message: "Shared instances cannot be cloned. Use cross-thread \
                          transfer via oxphp_async(fn() use (\\$this) {...})."
                    .into(),
                code: 0,
            })
        })
        // ── __construct(mixed $initial = null) ──────────────────────
        .method("__construct")
        .optional_param("initial", PhpType::Mixed, PhpValue::Null)
        .handler(|call| {
            // Extract initial as SharedValue via ValType dispatch.
            let initial_sv = match call.arg_type(0).unwrap_or(ValType::Null) {
                ValType::Long => SharedValue::Long(call.arg_long(0).unwrap_or(0)),
                ValType::Double => SharedValue::Double(call.arg_double(0).unwrap_or(0.0)),
                ValType::True => SharedValue::Bool(true),
                ValType::False => SharedValue::Bool(false),
                ValType::String => {
                    SharedValue::String(std::sync::Arc::from(call.arg_str(0).unwrap_or("")))
                }
                _ => SharedValue::Null,
            };

            let bytes = sv_to_portbuf(&initial_sv);
            let mut out_ptr: *const Entry = std::ptr::null();
            let rc =
                unsafe { oxphp_shared_mutex_create(bytes.as_ptr(), bytes.len(), &mut out_ptr) };
            super::counter::counter_rc_to_result(rc)?;

            let h = call.storage_mut::<SharedHandle>()?;
            h.entry_ptr = out_ptr;
            h.type_tag = SharedType::Mutex as u8;
            Ok(())
        })
        // ── withLock(callable $fn): mixed ───────────────────────────
        //   Forever (or fiber-cancel). Throws on contention only if
        //   the request fiber is cancelled (OperationTimeoutException
        //   from the SAPI layer or AsyncException directly).
        .method("withLock")
        .param("fn", PhpType::Callable)
        .returns(PhpType::Mixed)
        .handler(|call| invoke_mutex_with(call, /* timeout_ms = */ -1))
        // ── tryWithLock(callable $fn): mixed ────────────────────────
        //   Non-blocking. Throws ContentionException if the lock is held.
        .method("tryWithLock")
        .param("fn", PhpType::Callable)
        .returns(PhpType::Mixed)
        .handler(invoke_mutex_try_with)
        // ── withLockTimeout(callable $fn, int $ms): mixed ───────────
        //   Bounded. $ms > 0 enforced. Throws OperationTimeoutException
        //   on deadline expiry, DeadlockException on wait-for-cycle.
        .method("withLockTimeout")
        .param("fn", PhpType::Callable)
        .param("ms", PhpType::Int)
        .returns(PhpType::Mixed)
        .handler(|call| {
            let ms = read_positive_ms_arg(call, 1)?;
            invoke_mutex_with(call, ms)
        })
        // ── id(): int ───────────────────────────────────────────────
        .method("id")
        .returns(PhpType::Int)
        .handler(|call| {
            let h = call.storage::<SharedHandle>()?;
            if !h.is_initialized() {
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\UninitializedException".into(),
                    message: "uninitialised Shared wrapper".into(),
                    code: 0,
                });
            }
            let id =
                unsafe { crate::plugins::ox_shared::registry::oxphp_shared_entry_id(h.entry_ptr) };
            call.ret_long(id as i64);
            Ok(())
        })
        .build()?;

    Ok(())
}

fn invoke_mutex_with(
    call: &mut crate::bridge::call::NativeCall,
    timeout_ms: i64,
) -> Result<(), PhpError> {
    use crate::bridge::ffi;

    let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
    let callable_zv = unsafe { call.raw_arg_ptr(0) };

    let mut ret_buf: *mut u8 = std::ptr::null_mut();
    let mut ret_len: usize = 0;
    let rc = unsafe {
        oxphp_shared_mutex_with(
            entry_ptr,
            callable_zv,
            timeout_ms,
            &mut ret_buf,
            &mut ret_len,
        )
    };
    if rc != 0 {
        if !ret_buf.is_null() {
            unsafe { ffi::oxphp_portable_free(ret_buf) };
        }
        // SharedError::Generic (-1) when closure threw — EG(exception)
        // is already set; return a generic PhpError so the plugin
        // wrapper doesn't overwrite it.
        if rc == SharedError::Generic.code() {
            return Err(PhpError::Custom("Mutex::withLock closure threw".into()));
        }
        return Err(mutex_rc_to_phperr(rc));
    }

    if ret_buf.is_null() || ret_len == 0 {
        call.ret_null();
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(ret_buf, ret_len) };
        let entry: &Entry = unsafe { &*entry_ptr };
        match portbuf_to_sv(bytes).and_then(|raw| raw_to_owned(raw, entry.registry)) {
            Ok(sv) => write_value_to_retval(call, &sv)?,
            Err(_) => call.ret_null(),
        }
        unsafe { ffi::oxphp_portable_free(ret_buf) };
    }
    Ok(())
}

fn invoke_mutex_try_with(call: &mut crate::bridge::call::NativeCall) -> Result<(), PhpError> {
    use crate::bridge::ffi;

    let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
    let callable_zv = unsafe { call.raw_arg_ptr(0) };
    let mut ret_buf: *mut u8 = std::ptr::null_mut();
    let mut ret_len: usize = 0;
    let rc =
        unsafe { oxphp_shared_mutex_try_with(entry_ptr, callable_zv, &mut ret_buf, &mut ret_len) };

    // Contention path: the underlying try_with returns SharedError::Timeout
    // when the lock is held by another thread. Translate to the new
    // ContentionException — distinct from OperationTimeoutException
    // because the user did not pass a timeout budget.
    if rc == SharedError::Timeout.code() {
        if !ret_buf.is_null() {
            unsafe { ffi::oxphp_portable_free(ret_buf) };
        }
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\ContentionException".into(),
            message: "Mutex::tryWithLock: lock is held".into(),
            code: 0,
        });
    }
    if rc != 0 {
        if !ret_buf.is_null() {
            unsafe { ffi::oxphp_portable_free(ret_buf) };
        }
        if rc == SharedError::Generic.code() {
            return Err(PhpError::Custom("Mutex::tryWithLock closure threw".into()));
        }
        return Err(mutex_rc_to_phperr(rc));
    }

    if ret_buf.is_null() || ret_len == 0 {
        call.ret_null();
    } else {
        let bytes = unsafe { std::slice::from_raw_parts(ret_buf, ret_len) };
        let entry: &Entry = unsafe { &*entry_ptr };
        match portbuf_to_sv(bytes).and_then(|raw| raw_to_owned(raw, entry.registry)) {
            Ok(sv) => write_value_to_retval(call, &sv)?,
            Err(_) => call.ret_null(),
        }
        unsafe { ffi::oxphp_portable_free(ret_buf) };
    }
    Ok(())
}

fn mutex_rc_to_phperr(rc: c_int) -> PhpError {
    let class = match rc {
        -2 => "OxPHP\\Shared\\StaleHandleException",
        -3 => "OxPHP\\Shared\\TypeException",
        -4 => "OxPHP\\Shared\\CapacityException",
        // Timeout from withLockTimeout's bounded wait → OperationTimeoutException.
        -7 => "OxPHP\\Shared\\OperationTimeoutException",
        -8 => "OxPHP\\Shared\\DeadlockException",
        -10 => "OxPHP\\Shared\\UninitializedException",
        // -99 = Rust panic crossed FFI → corrupted mutex.
        -99 => "OxPHP\\Shared\\CorruptedMutexException",
        _ => "OxPHP\\Shared\\SharedException",
    };
    PhpError::Exception {
        class: class.to_string(),
        message: read_last_error_message(),
        code: 0,
    }
}

fn write_value_to_retval(
    call: &mut crate::bridge::call::NativeCall,
    v: &SharedValue,
) -> Result<(), PhpError> {
    match v {
        SharedValue::Null => call.ret_null(),
        SharedValue::Bool(b) => call.ret_bool(*b),
        SharedValue::Long(l) => call.ret_long(*l),
        SharedValue::Double(d) => call.ret_double(*d),
        SharedValue::String(s) => call.ret_str(s),
        SharedValue::Bytes(b) => call.ret_bytes(b),
        _ => {
            return Err(PhpError::Exception {
                class: "OxPHP\\Shared\\TypeException".into(),
                message: "Mutex does not yet support array/nested-shared return".into(),
                code: 0,
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corruption_round_trip() {
        let m = MutexInner::new(SharedValue::Long(10));
        assert!(!m.is_corrupted());
        m.mark_corrupted();
        assert!(m.is_corrupted());
        // No clear path — corruption is sticky by design.
    }

    #[test]
    fn snapshot_returns_value() {
        let m = MutexInner::new(SharedValue::Long(42));
        match m.try_snapshot() {
            SharedValue::Long(42) => {}
            other => panic!("wrong snapshot: {other:?}"),
        }
    }
}
