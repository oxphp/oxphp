//! Shared\Once — lazy-initialisation guard.
//!
//! Exposes `trySet` (scalar-only) and `init(callable $factory)` backed
//! by `oxphp_shared_invoke_0_portbuf`.

use std::cell::RefCell;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use smallvec::SmallVec;

use crate::bridge::types::ValType;
use crate::plugin::types::{MagicMethod, PhpType};
use crate::plugin::{PhpError, PluginContext, PluginError};
use crate::plugins::ox_shared::error::{ffi_entry, set_last_error, SharedError};
use crate::plugins::ox_shared::handle::SharedHandle;
use crate::plugins::ox_shared::registry::{registry, SharedInner, SharedType};
use crate::plugins::ox_shared::value::SharedValue;

// Per-thread set of Once ids currently inside `init(factory)` on
// this thread. Used to detect recursive Once::init / Once::get
// from within the factory, which would deadlock against the
// parking_lot::Mutex<()> init lock.
thread_local! {
    static HELD_ONCES: RefCell<SmallVec<[u64; 4]>> = const {
        RefCell::new(SmallVec::new_const())
    };
}

fn push_once_held(id: u64) -> Result<(), SharedError> {
    HELD_ONCES.with(|h| {
        let mut v = h.borrow_mut();
        if v.contains(&id) {
            set_last_error(format!(
                "recursive Once::init on id={} from within its own factory (deadlock avoided)",
                id
            ));
            return Err(SharedError::Deadlock);
        }
        v.push(id);
        Ok(())
    })
}

fn pop_once_held(id: u64) {
    HELD_ONCES.with(|h| {
        let mut v = h.borrow_mut();
        if let Some(pos) = v.iter().rposition(|x| *x == id) {
            v.swap_remove(pos);
        }
    });
}

/// RAII guard that pops HELD_ONCES on drop (panic-safe).
struct OncePopGuard(u64);
impl Drop for OncePopGuard {
    fn drop(&mut self) {
        pop_once_held(self.0);
    }
}

pub struct OnceInner {
    value: OnceLock<SharedValue>,
    set_lock: Mutex<()>,
    initialized: AtomicBool,
}

impl OnceInner {
    pub fn new() -> Self {
        Self {
            value: OnceLock::new(),
            set_lock: Mutex::new(()),
            initialized: AtomicBool::new(false),
        }
    }

    pub fn get(&self) -> Option<SharedValue> {
        self.value.get().cloned()
    }

    pub fn is_initialized(&self) -> bool {
        self.initialized.load(Ordering::Acquire)
    }

    /// Runs `factory_rs` if not yet initialised, then stores and returns
    /// the result. Concurrent callers block on set_lock; the first to
    /// win stores its value and releases all waiters.
    ///
    /// On `factory_rs` error, the Once stays uninitialised — a subsequent
    /// `init_or_run_factory` call on any thread may retry.
    pub fn init_or_run_factory<F>(&self, factory_rs: F) -> Result<SharedValue, SharedError>
    where
        F: FnOnce() -> Result<SharedValue, SharedError>,
    {
        if let Some(v) = self.value.get() {
            return Ok(v.clone());
        }
        let _guard = self.set_lock.lock();
        if let Some(v) = self.value.get() {
            return Ok(v.clone());
        }
        let new_value = factory_rs()?;
        let _ = self.value.set(new_value.clone());
        self.initialized
            .store(true, std::sync::atomic::Ordering::Release);
        Ok(new_value)
    }

    /// Attempts to set the value. Returns true if we wrote it, false if
    /// another writer got there first.
    pub fn try_set(&self, v: SharedValue) -> bool {
        if self.is_initialized() {
            return false;
        }
        let _guard = self.set_lock.lock();
        if self.is_initialized() {
            return false;
        }
        // OnceLock::set is itself atomic but we hold the mutex for
        // determinism across the initialized flag read/store.
        let _ = self.value.set(v);
        self.initialized.store(true, Ordering::Release);
        true
    }
}

impl Default for OnceInner {
    fn default() -> Self {
        Self::new()
    }
}

impl SharedInner for OnceInner {
    fn type_tag(&self) -> SharedType {
        SharedType::Once
    }
    fn debug_snapshot(&self) -> SharedValue {
        self.value.get().cloned().unwrap_or(SharedValue::Null)
    }
    fn mem_bytes(&self) -> usize {
        16 + self.value.get().map_or(0, |v| v.mem_bytes())
    }
    fn on_drop(&self) {}
    fn on_shutdown_notify(&self) {}
}

// Helper trait for downcasting Arc<dyn SharedInner> to &OnceInner.
// Implemented on `dyn SharedInner` (not `+ Send + Sync`) to match Entry.inner's
// actual trait-object type used by counter.rs and flag.rs.
pub trait SharedInnerOnceExt {
    fn as_any_once(&self) -> Option<&OnceInner>;
}

impl SharedInnerOnceExt for dyn SharedInner {
    fn as_any_once(&self) -> Option<&OnceInner> {
        if self.type_tag() == SharedType::Once {
            // SAFETY: SharedType::Once guarantees the concrete type is OnceInner.
            // Casting a `*const dyn SharedInner` fat pointer to `*const OnceInner`
            // yields the data pointer, which is the address of the OnceInner
            // allocation. Sound as long as SharedType::Once is only ever used with
            // OnceInner — enforced by the sole insertion site in
            // oxphp_shared_once_create.
            Some(unsafe { &*(self as *const dyn SharedInner as *const OnceInner) })
        } else {
            None
        }
    }
}

// ─── FFI ──────────────────────────────────────────────────────────────

/// # Safety
/// `out_id` must be valid for writes of u64 if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_once_create(out_id: *mut u64) -> c_int {
    if out_id.is_null() {
        set_last_error("out_id null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let id = reg.insert(SharedType::Once, Arc::new(OnceInner::new()))?;
        unsafe { *out_id = id };
        Ok(())
    })
}

/// # Safety
/// `out` must be valid for writes of c_int if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_once_is_initialized(id: u64, out: *mut c_int) -> c_int {
    if out.is_null() {
        set_last_error("out null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let inner = entry.inner.as_any_once().ok_or(SharedError::Type)?;
        let init = inner.is_initialized();
        reg.record_op(id);
        unsafe { *out = init as c_int };
        Ok(())
    })
}

// ─── Class registration ───────────────────────────────────────────────

pub fn register_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_class("OxPHP\\Shared\\Once")
        .implements("OxPHP\\Shared\\Shareable")
        .with_storage(|| SharedHandle::new(SharedType::Once))
        .magic(MagicMethod::Clone)
        .handler(|_call| {
            Err(PhpError::Exception {
                class: "OxPHP\\Shared\\Exception".to_string(),
                message: "Shared instances cannot be cloned. Use cross-thread \
                          transfer via oxphp_async(fn() use ($this) {...})."
                    .to_string(),
                code: 0,
            })
        })
        .method("__construct")
        .handler(|call| {
            let mut out_id: u64 = 0;
            let rc = unsafe { oxphp_shared_once_create(&mut out_id) };
            super::counter::counter_rc_to_result(rc)?;
            let h = call.storage_mut::<SharedHandle>()?;
            h.shared_id = out_id;
            h.type_tag = SharedType::Once as u8;
            Ok(())
        })
        .method("get")
        .returns(PhpType::Mixed)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let reg = registry();
            let entry = reg.lookup(id).map_err(|e| err_to_phperr(e, id))?;
            let inner = entry
                .inner
                .as_any_once()
                .ok_or_else(|| type_error("entry is not a Once"))?;
            match inner.get() {
                Some(v) => write_value_to_retval(call, &v)?,
                None => call.ret_null(),
            }
            reg.record_op(id);
            Ok(())
        })
        .method("isInitialized")
        .returns(PhpType::Bool)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let mut out: c_int = 0;
            let rc = unsafe { oxphp_shared_once_is_initialized(id, &mut out) };
            super::counter::counter_rc_to_result(rc)?;
            call.ret_bool(out != 0);
            Ok(())
        })
        .method("trySet")
        .param("value", PhpType::Mixed)
        .returns(PhpType::Bool)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let reg = registry();
            let entry = reg.lookup(id).map_err(|e| err_to_phperr(e, id))?;
            let inner = entry
                .inner
                .as_any_once()
                .ok_or_else(|| type_error("entry is not a Once"))?;

            // Read argument as a SharedValue (scalar-only).
            let sv = read_arg_as_shared_value(call, 0)?;
            let winner = inner.try_set(sv);
            reg.record_op(id);
            call.ret_bool(winner);
            Ok(())
        })
        .method("init")
        .param("factory", PhpType::Callable)
        .returns(PhpType::Mixed)
        .handler(|call| {
            use crate::bridge::ffi;
            use crate::plugins::ox_shared::value::portbuf_to_sv;

            let id = call.storage::<SharedHandle>()?.shared_id;

            // Reentrance guard BEFORE taking the init_lock.
            push_once_held(id).map_err(|e| err_to_phperr(e, id))?;
            let _pop = OncePopGuard(id);

            // Look up the Once entry.
            let reg = registry();
            let entry = reg.lookup(id).map_err(|e| err_to_phperr(e, id))?;
            let inner = entry
                .inner
                .as_any_once()
                .ok_or_else(|| type_error("entry is not a Once"))?;

            // Fast path: already initialised; write cached value, skip factory.
            if let Some(v) = inner.get() {
                write_value_to_retval(call, &v)?;
                reg.record_op(id);
                return Ok(());
            }

            // Slow path: invoke factory via C shim, capture portbuf bytes.
            let callable_zv = unsafe { call.raw_arg_ptr(0) };
            let sv = inner.init_or_run_factory(|| {
                let mut out_buf: *mut u8 = std::ptr::null_mut();
                let mut out_len: usize = 0;
                let rc = unsafe {
                    ffi::oxphp_shared_invoke_0_portbuf(callable_zv, &mut out_buf, &mut out_len)
                };
                match rc {
                    x if x == ffi::OXPHP_SHARED_INVOKE_OK => {
                        let bytes = unsafe { std::slice::from_raw_parts(out_buf, out_len) };
                        let sv = portbuf_to_sv(bytes);
                        unsafe { ffi::oxphp_portable_free(out_buf) };
                        sv
                    }
                    x if x == ffi::OXPHP_SHARED_INVOKE_PHP_THREW => {
                        if !out_buf.is_null() {
                            unsafe { ffi::oxphp_portable_free(out_buf) };
                        }
                        // EG(exception) is set; surface via the plugin
                        // framework's existing PhpError pathway.
                        Err(SharedError::Generic)
                    }
                    _ => {
                        if !out_buf.is_null() {
                            unsafe { ffi::oxphp_portable_free(out_buf) };
                        }
                        set_last_error("Once::init: factory is not a valid callable");
                        Err(SharedError::Type)
                    }
                }
            });

            match sv {
                Ok(v) => {
                    write_value_to_retval(call, &v)?;
                    reg.record_op(id);
                    Ok(())
                }
                Err(SharedError::Generic) => {
                    // PHP exception already pending; plugin framework
                    // will see EG(exception) and surface it.
                    Err(PhpError::Custom("Once::init factory threw".into()))
                }
                Err(e) => Err(err_to_phperr(e, id)),
            }
        })
        .method("id")
        .returns(PhpType::Int)
        .handler(|call| {
            let h = call.storage::<SharedHandle>()?;
            if !h.is_initialized() {
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\UninitializedException".to_string(),
                    message: "uninitialised Shared wrapper".to_string(),
                    code: 0,
                });
            }
            call.ret_long(h.shared_id as i64);
            Ok(())
        })
        .build()?;
    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────

fn err_to_phperr(e: SharedError, _id: u64) -> PhpError {
    let class = match e {
        SharedError::StaleHandle => "OxPHP\\Shared\\StaleHandleException",
        SharedError::Type => "OxPHP\\Shared\\TypeException",
        SharedError::CapacityExceeded => "OxPHP\\Shared\\CapacityException",
        SharedError::Uninitialized => "OxPHP\\Shared\\UninitializedException",
        SharedError::Deadlock => "OxPHP\\Shared\\DeadlockException",
        SharedError::Timeout => "OxPHP\\Shared\\TimeoutException",
        SharedError::Poisoned => "OxPHP\\Shared\\PoisonedException",
        SharedError::Closed => "OxPHP\\Shared\\ClosedException",
        SharedError::Cycle => "OxPHP\\Shared\\CycleException",
        _ => "OxPHP\\Shared\\Exception",
    };
    PhpError::Exception {
        class: class.to_string(),
        message: e.to_string(),
        code: 0,
    }
}

fn type_error(msg: &str) -> PhpError {
    PhpError::Exception {
        class: "OxPHP\\Shared\\TypeException".to_string(),
        message: msg.to_string(),
        code: 0,
    }
}

fn read_arg_as_shared_value(
    call: &crate::bridge::call::NativeCall,
    idx: u32,
) -> Result<SharedValue, PhpError> {
    let t = call.arg_type(idx)?;
    Ok(match t {
        ValType::Null => SharedValue::Null,
        ValType::True => SharedValue::Bool(true),
        ValType::False => SharedValue::Bool(false),
        ValType::Long => SharedValue::Long(call.arg_long(idx)?),
        ValType::Double => SharedValue::Double(call.arg_double(idx)?),
        ValType::String => SharedValue::String(Arc::from(call.arg_str(idx)?)),
        _ => {
            return Err(type_error(
                "Once::trySet accepts scalars only (null, bool, int, float, string)",
            ));
        }
    })
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
            return Err(type_error(
                "Once does not support array/nested-shared values",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_empty() {
        let o = OnceInner::new();
        assert!(o.get().is_none());
        assert!(!o.is_initialized());
    }

    #[test]
    fn try_set_stores_once() {
        let o = OnceInner::new();
        assert!(o.try_set(SharedValue::Long(42)));
        assert!(!o.try_set(SharedValue::Long(99))); // already set
        match o.get().unwrap() {
            SharedValue::Long(42) => {}
            _ => panic!("wrong value"),
        }
        assert!(o.is_initialized());
    }

    #[test]
    fn try_set_bool_and_string() {
        let o = OnceInner::new();
        assert!(o.try_set(SharedValue::String(Arc::from("hello"))));
        let got = o.get().unwrap();
        if let SharedValue::String(s) = got {
            assert_eq!(&*s, "hello");
        } else {
            panic!("wrong shape");
        }
    }
}
