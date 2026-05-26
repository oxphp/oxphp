//! Shared\Once — lazy-initialisation guard.
//!
//! A four-state automaton (Uninitialized/Pending/Ready/Poisoned). Exposes
//! `trySet` (push), `getOrInit(callable $factory)` (pull, race-free), and
//! `status()`. Factory invocation is backed by `oxphp_shared_invoke_0_portbuf`.

use std::cell::RefCell;
use std::os::raw::c_int;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, OnceLock};

use parking_lot::Mutex;
use smallvec::SmallVec;

use crate::bridge::types::ValType;
use crate::plugin::types::{MagicMethod, PhpType};
use crate::plugin::{PhpError, PluginContext, PluginError};
use crate::plugins::ox_shared::error::{
    ffi_entry, read_last_error_message, set_last_error, SharedError,
};
use crate::plugins::ox_shared::handle::SharedHandle;
use crate::plugins::ox_shared::registry::{
    registry, Entry, SharedInner, SharedRegistry, SharedType, ENTRY_MAGIC,
};
use std::sync::Weak;
use crate::plugins::ox_shared::value::SharedValue;

// Per-thread set of Once ids currently inside `getOrInit(factory)` on
// this thread. Used to detect a recursive Once::getOrInit on the same
// cell from within its own factory, which would deadlock against the
// parking_lot::Mutex<()> set lock. `get()` / `status()` take no lock and
// are not tracked here.
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
                "recursive Once::getOrInit on id={} from within its own factory (deadlock avoided)",
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

pub(crate) const ST_UNINIT: u8 = 0;
pub(crate) const ST_PENDING: u8 = 1;
pub(crate) const ST_READY: u8 = 2;
pub(crate) const ST_POISONED: u8 = 3;

#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub enum OnceFailureMode {
    Reset,
    Poison,
}

impl OnceFailureMode {
    /// Decode the backing int of `Once\FailureMode` (Reset=0, Poison=1).
    /// Any other value is treated as Reset (the safe default).
    pub fn from_i64(v: i64) -> Self {
        match v {
            1 => OnceFailureMode::Poison,
            _ => OnceFailureMode::Reset,
        }
    }
}

/// Captured PHP-exception details from a failed factory. Owned strings so
/// the info survives cross-thread (the original zval is thread-local).
#[derive(Clone, Debug)]
pub struct PoisonInfo {
    pub class: String,
    pub message: String,
    pub code: i64,
}

impl PartialEq for PoisonInfo {
    fn eq(&self, other: &Self) -> bool {
        self.class == other.class && self.message == other.message && self.code == other.code
    }
}
impl Eq for PoisonInfo {}

/// Error returned by the factory closure passed to [`OnceInner::get_or_init`].
///
/// Mapping to [`OnceFailureMode`]:
/// - `Threw` and `NotSerialisable` are *factory failures* — the call
///   site was correct, but execution did not produce a usable value.
///   They honour the configured `FailureMode` (Reset → retryable,
///   Poison → terminal).
/// - `Invalid` is a *programmer mistake at the call site* — the
///   argument was not callable at all. It always resets so a follow-up
///   call with a real callable succeeds, regardless of `FailureMode`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OnceFactoryError {
    /// The PHP factory threw; carries the captured exception info.
    Threw(PoisonInfo),
    /// The argument was not a valid callable.
    Invalid,
    /// The factory ran and returned, but its value cannot be stored in
    /// shared memory (closure / resource / non-`Shareable` object).
    NotSerialisable,
}

/// RAII guard that resets the state from `Pending` to `Uninitialized` if a
/// factory closure panics mid-init. Disarmed on every normal return so the
/// explicit match arms own the terminal transition.
///
/// Note: this resets (retryable) even under `FailureMode::Poison`. That is
/// intentional — `Poison` is the contract for a PHP factory *exception*
/// (`OXPHP_SHARED_INVOKE_PHP_THREW`), a normal application outcome. A Rust
/// panic is an internal "should not happen" fault caught by the worker's
/// `catch_unwind`; it does not carry poison info and must not permanently
/// disable the cell. The decode paths return `Result`, so this is nearly
/// unreachable in practice.
struct PendingResetGuard<'a> {
    state: &'a AtomicU8,
    armed: bool,
}

impl Drop for PendingResetGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            self.state.store(ST_UNINIT, Ordering::Release);
        }
    }
}

pub struct OnceInner {
    state: AtomicU8,
    value: OnceLock<SharedValue>,
    poison: Mutex<Option<PoisonInfo>>,
    set_lock: Mutex<()>,
    failure_mode: OnceFailureMode,
    /// Back-reference to the owning Entry, set in `bind_entry` after the
    /// registry insert. Lets the post-init writes call
    /// `Entry::adjust_mem_bytes` so `total_bytes` reflects the stored
    /// payload size (otherwise it would stay at the empty-Once skeleton
    /// and silently bypass `SHARED_MAX_BYTES` caps).
    self_entry: OnceLock<Weak<Entry>>,
}

impl OnceInner {
    pub fn new() -> Self {
        Self::with_mode(OnceFailureMode::Reset)
    }

    pub fn with_mode(failure_mode: OnceFailureMode) -> Self {
        Self {
            state: AtomicU8::new(ST_UNINIT),
            value: OnceLock::new(),
            poison: Mutex::new(None),
            set_lock: Mutex::new(()),
            failure_mode,
            self_entry: OnceLock::new(),
        }
    }

    /// Wire the back-reference to the owning Entry. Called by
    /// `oxphp_shared_once_create` right after `registry.insert()`.
    pub fn bind_entry(&self, weak: Weak<Entry>) {
        let _ = self.self_entry.set(weak);
    }

    /// Book `value`'s contribution against `Entry::mem_bytes` and the
    /// registry's `total_bytes`. No-op before `bind_entry` (e.g. unit
    /// tests that exercise `OnceInner::new()` standalone).
    fn track_set_delta(&self, value: &SharedValue) {
        let Some(weak) = self.self_entry.get() else {
            return;
        };
        let Some(entry) = weak.upgrade() else { return };
        let delta = value.mem_bytes() as isize;
        if delta != 0 {
            entry.adjust_mem_bytes(delta);
        }
    }

    pub fn state(&self) -> u8 {
        self.state.load(Ordering::Acquire)
    }

    /// Value if and only if the cell is Ready.
    pub fn get(&self) -> Option<SharedValue> {
        if self.state() == ST_READY {
            self.value.get().cloned()
        } else {
            None
        }
    }

    pub fn poison_info(&self) -> Option<PoisonInfo> {
        self.poison.lock().clone()
    }

    /// Push-model write. `Ok(true)` stored, `Ok(false)` already
    /// Ready/Pending, `Err(Poisoned)` if poisoned.
    pub fn try_set(&self, v: SharedValue) -> Result<bool, SharedError> {
        match self.state() {
            ST_POISONED => return Err(SharedError::Poisoned),
            ST_READY | ST_PENDING => return Ok(false),
            _ => {}
        }
        let _guard = self.set_lock.lock();
        match self.state() {
            ST_POISONED => return Err(SharedError::Poisoned),
            ST_READY | ST_PENDING => return Ok(false),
            _ => {}
        }
        self.track_set_delta(&v);
        let _ = self.value.set(v);
        self.state.store(ST_READY, Ordering::Release);
        Ok(true)
    }

    /// Pull-model get-or-init. Orchestrates the state machine; `factory`
    /// runs at most once for the winning caller. On factory `Threw`, the
    /// failure mode decides reset vs poison; either way returns
    /// `Err(SharedError::Generic)` so the handler surfaces the pending PHP
    /// exception to the current caller.
    pub fn get_or_init<F>(&self, factory: F) -> Result<SharedValue, SharedError>
    where
        F: FnOnce() -> Result<SharedValue, OnceFactoryError>,
    {
        match self.state() {
            ST_READY => return Ok(self.value.get().cloned().expect("ready has value")),
            ST_POISONED => return Err(SharedError::Poisoned),
            _ => {}
        }
        // `set_lock` is held across the whole factory run. The HELD_ONCES
        // guard catches single-thread reentrancy (factory re-enters the same
        // cell) and turns it into a DeadlockException. It does NOT catch a
        // cross-thread *cycle*: thread 1 initialising cell A whose factory
        // calls B->getOrInit() while thread 2 initialises B whose factory
        // calls A->getOrInit() will deadlock on the two set_locks. Only
        // single-thread reentrancy is contractually safe (see docs); avoid
        // factory graphs that lock two cells in opposite orders across threads.
        let _guard = self.set_lock.lock();
        match self.state() {
            ST_READY => return Ok(self.value.get().cloned().expect("ready has value")),
            ST_POISONED => return Err(SharedError::Poisoned),
            _ => {}
        }
        // We are the initializer for this round.
        self.state.store(ST_PENDING, Ordering::Release);
        // If `factory()` panics, unwind skips the arms below and would
        // strand the cell in `Pending` forever. This guard resets it to
        // `Uninitialized` (retryable) on unwind — matching the pre-state-
        // machine behaviour. It is disarmed on every normal return path.
        let mut pending_guard = PendingResetGuard {
            state: &self.state,
            armed: true,
        };
        let outcome = factory();
        pending_guard.armed = false;
        match outcome {
            Ok(v) => {
                self.track_set_delta(&v);
                let _ = self.value.set(v.clone());
                self.state.store(ST_READY, Ordering::Release);
                Ok(v)
            }
            Err(OnceFactoryError::Threw(info)) => {
                match self.failure_mode {
                    OnceFailureMode::Reset => {
                        self.state.store(ST_UNINIT, Ordering::Release);
                    }
                    OnceFailureMode::Poison => {
                        // Store poison info BEFORE flipping state so a racing
                        // reader that sees Poisoned always finds the info.
                        *self.poison.lock() = Some(info);
                        self.state.store(ST_POISONED, Ordering::Release);
                    }
                }
                Err(SharedError::Generic)
            }
            Err(OnceFactoryError::NotSerialisable) => {
                // Factory ran to completion and produced a value the
                // shared layer cannot store (closure / resource /
                // non-Shareable object). That is a *factory failure* in
                // the same operational sense as Threw — the same code
                // will keep failing on retry — so honour the
                // FailureMode contract: Reset stays retryable, Poison
                // makes the cell terminal. Synthesise a PoisonInfo with
                // the TypeException class so later callers see a
                // PoisonedException carrying an explanatory message.
                match self.failure_mode {
                    OnceFailureMode::Reset => {
                        self.state.store(ST_UNINIT, Ordering::Release);
                    }
                    OnceFailureMode::Poison => {
                        *self.poison.lock() = Some(PoisonInfo {
                            class: "OxPHP\\Shared\\TypeException".to_string(),
                            message: "factory returned a non-serialisable value \
                                 (closure / resource / non-Shareable object)"
                                .to_string(),
                            code: 0,
                        });
                        self.state.store(ST_POISONED, Ordering::Release);
                    }
                }
                Err(SharedError::Type)
            }
            Err(OnceFactoryError::Invalid) => {
                // The callable argument itself was unusable (NULL,
                // fcall_init failure). That is a programmer mistake on
                // the *call site*, not a runtime factory failure — the
                // next call with a real callable should succeed — so
                // reset regardless of FailureMode. Aligns with the
                // PendingResetGuard policy (Rust panic = internal
                // fault, never poison).
                self.state.store(ST_UNINIT, Ordering::Release);
                Err(SharedError::Type)
            }
        }
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
    fn as_any(&self) -> &dyn std::any::Any {
        self
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
        self.as_any().downcast_ref::<OnceInner>()
    }
}

// ─── FFI ──────────────────────────────────────────────────────────────

/// # Safety
/// `out_ptr` must be valid for writes of `*const Entry` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_once_create(
    failure_mode: c_int,
    out_ptr: *mut *const Entry,
) -> c_int {
    if out_ptr.is_null() {
        set_last_error("out_ptr is null");
        return SharedError::Generic.code();
    }
    let mode = OnceFailureMode::from_i64(failure_mode as i64);
    ffi_entry(|| {
        let reg = registry();
        let inner = Arc::new(OnceInner::with_mode(mode));
        let arc = reg.insert(SharedType::Once, inner.clone())?;
        // Wire the inner's back-reference so post-init writes can adjust
        // total_bytes via Entry::adjust_mem_bytes.
        inner.bind_entry(Arc::downgrade(&arc));
        // SAFETY: out_ptr checked non-null above.
        unsafe { *out_ptr = Arc::into_raw(arc) };
        Ok(())
    })
}

// ─── Class registration ───────────────────────────────────────────────

/// Register `OxPHP\Shared\Once\Status` (unbacked) and
/// `OxPHP\Shared\Once\FailureMode` (backed int). Must run before
/// `register_class` so the class's param/return metadata can resolve them.
pub fn register_enums(ctx: &mut PluginContext) -> Result<(), PluginError> {
    use crate::plugin::types::PhpValue;

    ctx.register_enum("OxPHP\\Shared\\Once\\Status")
        .case("Uninitialized")
        .case("Pending")
        .case("Ready")
        .case("Poisoned")
        .build()?;

    ctx.register_enum("OxPHP\\Shared\\Once\\FailureMode")
        .backed_by(PhpType::Int)
        .case_value("Reset", PhpValue::Int(0))
        .case_value("Poison", PhpValue::Int(1))
        .build()?;

    Ok(())
}

pub fn register_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    use crate::plugin::types::PhpValue;

    ctx.register_class("OxPHP\\Shared\\Once")
        .implements("OxPHP\\Shared\\Shareable")
        .with_storage(|| SharedHandle::new(SharedType::Once))
        .magic(MagicMethod::Clone)
        .handler(|_call| {
            Err(PhpError::Exception {
                class: "OxPHP\\Shared\\SharedException".to_string(),
                message: "Shared instances cannot be cloned. Use cross-thread \
                          transfer via oxphp_async(fn() use ($this) {...})."
                    .to_string(),
                code: 0,
            })
        })
        // __construct(Once\FailureMode $onFactoryError = Reset)
        .method("__construct")
        .optional_param(
            "onFactoryError",
            PhpType::Enum("OxPHP\\Shared\\Once\\FailureMode".to_string()),
            PhpValue::ConstExpr("\\OxPHP\\Shared\\Once\\FailureMode::Reset".to_string()),
        )
        .handler(|call| {
            // Absent arg => Reset; present => decode backing int.
            let mode_i = if call.argc() > 0 {
                call.arg_enum_long(0)?
            } else {
                0
            };
            let mut out_ptr: *const Entry = std::ptr::null();
            let rc = unsafe { oxphp_shared_once_create(mode_i as c_int, &mut out_ptr) };
            super::counter::counter_rc_to_result(rc)?;
            let h = call.storage_mut::<SharedHandle>()?;
            h.entry_ptr = out_ptr;
            h.type_tag = SharedType::Once as u8;
            Ok(())
        })
        // get(): T — throws on uninit/pending/poisoned
        .method("get")
        .returns(PhpType::Mixed)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            if entry_ptr.is_null() {
                return Err(err_to_phperr(SharedError::StaleHandle, 0));
            }
            // SAFETY: entry_ptr non-null and, per the handle contract, a
            // live Arc::into_raw pointer.
            let entry: &Entry = unsafe { &*entry_ptr };
            debug_assert_eq!(entry.magic, ENTRY_MAGIC, "Once::get on freed Entry");
            let inner = entry
                .inner
                .as_any_once()
                .ok_or_else(|| type_error("entry is not a Once"))?;
            // Count the access regardless of outcome — the per-entry op
            // counter measures traffic, not success (matches Channel recv and
            // the pre-redesign Once, which counted empty reads too).
            entry.registry.record_op(entry);
            match inner.state() {
                ST_READY => {
                    let v = inner.get().expect("ready has value");
                    write_value_to_retval(call, &v)?;
                }
                ST_POISONED => return Err(poisoned_error(inner)),
                // Both empty and in-flight throw UninitializedException (one
                // contract), but the message names the actual state for DX.
                state => {
                    let message = if state == ST_PENDING {
                        "Once::get() while a factory is initialising this cell on \
                         another thread (Pending) \u{2014} use getOrInit($factory) to \
                         wait for the value"
                    } else {
                        "Once::get() on an uninitialised cell \u{2014} use \
                         getOrInit($factory) or check status()"
                    };
                    return Err(PhpError::Exception {
                        class: "OxPHP\\Shared\\UninitializedException".to_string(),
                        message: message.to_string(),
                        code: 0,
                    });
                }
            }
            Ok(())
        })
        // status(): Once\Status — never throws
        .method("status")
        .returns(PhpType::Enum("OxPHP\\Shared\\Once\\Status".to_string()))
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            if entry_ptr.is_null() {
                return Err(err_to_phperr(SharedError::StaleHandle, 0));
            }
            // SAFETY: live Arc::into_raw pointer per the handle contract.
            let entry: &Entry = unsafe { &*entry_ptr };
            debug_assert_eq!(entry.magic, ENTRY_MAGIC, "Once::status on freed Entry");
            let inner = entry
                .inner
                .as_any_once()
                .ok_or_else(|| type_error("entry is not a Once"))?;
            let case = match inner.state() {
                ST_UNINIT => "Uninitialized",
                ST_PENDING => "Pending",
                ST_READY => "Ready",
                ST_POISONED => "Poisoned",
                other => return Err(PhpError::Custom(format!("Once: bad state {other}"))),
            };
            entry.registry.record_op(entry);
            emit_status_case(call, case)
        })
        // trySet(T $value): bool
        .method("trySet")
        .param("value", PhpType::Mixed)
        .returns(PhpType::Bool)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            if entry_ptr.is_null() {
                return Err(err_to_phperr(SharedError::StaleHandle, 0));
            }
            // SAFETY: entry_ptr non-null and, per the handle contract, a
            // live Arc::into_raw pointer.
            let entry: &Entry = unsafe { &*entry_ptr };
            debug_assert_eq!(entry.magic, ENTRY_MAGIC, "Once::trySet on freed Entry");
            let inner = entry
                .inner
                .as_any_once()
                .ok_or_else(|| type_error("entry is not a Once"))?;

            // Cheap pre-check: a cell that is already Ready/Pending can't
            // accept a write, and a Poisoned cell throws — bail before
            // serialising a potentially large argument. try_set re-checks
            // authoritatively under the lock, so this is a pure optimisation
            // (a racing transition into Uninit just falls through to it).
            match inner.state() {
                ST_POISONED => {
                    entry.registry.record_op(entry);
                    return Err(poisoned_error(inner));
                }
                ST_READY | ST_PENDING => {
                    entry.registry.record_op(entry);
                    call.ret_bool(false);
                    return Ok(());
                }
                _ => {}
            }

            // Read the argument as a full SharedValue (scalars fast-path,
            // arrays / nested Shared via the portbuf codec).
            let sv = read_arg_as_shared_value(call, 0, entry.registry)?;
            let result = inner.try_set(sv);
            // Count the access regardless of outcome (won/lost/poisoned).
            entry.registry.record_op(entry);
            match result {
                Ok(stored) => {
                    call.ret_bool(stored);
                    Ok(())
                }
                Err(SharedError::Poisoned) => Err(poisoned_error(inner)),
                Err(e) => Err(err_to_phperr(e, 0)),
            }
        })
        // getOrInit(callable $factory): T
        .method("getOrInit")
        .param("factory", PhpType::Callable)
        .returns(PhpType::Mixed)
        .handler(|call| {
            use crate::bridge::ffi;
            use crate::plugins::ox_shared::value::{portbuf_to_sv, raw_to_owned};

            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            if entry_ptr.is_null() {
                return Err(err_to_phperr(SharedError::StaleHandle, 0));
            }
            // SAFETY: entry_ptr non-null and, per the handle contract, a
            // live Arc::into_raw pointer.
            let entry: &Entry = unsafe { &*entry_ptr };
            debug_assert_eq!(entry.magic, ENTRY_MAGIC, "Once::getOrInit on freed Entry");
            let id = entry.id;

            // Reentrance guard BEFORE taking the init lock.
            push_once_held(id).map_err(|e| err_to_phperr(e, id))?;
            let _pop = OncePopGuard(id);

            let inner = entry
                .inner
                .as_any_once()
                .ok_or_else(|| type_error("entry is not a Once"))?;

            let callable_zv = unsafe { call.raw_arg_ptr(0) };
            let result = inner.get_or_init(|| {
                let mut out_buf: *mut u8 = std::ptr::null_mut();
                let mut out_len: usize = 0;
                // If the factory returns a Shared\* wrapper, the C shim
                // retains its Entry across the wrapper drop so the lookup
                // path below cannot race against Entry::drop. RetainGuard
                // releases the +1 when this closure exits, by which point
                // the SharedValue::Shared(owned) (if any) has its own Arc.
                let mut retained: *const std::os::raw::c_void = std::ptr::null();
                let rc = unsafe {
                    ffi::oxphp_shared_invoke_0_portbuf(
                        callable_zv,
                        &mut out_buf,
                        &mut out_len,
                        &mut retained,
                    )
                };
                struct RetainGuard(*const std::os::raw::c_void);
                impl Drop for RetainGuard {
                    fn drop(&mut self) {
                        if !self.0.is_null() {
                            unsafe {
                                drop(std::sync::Arc::from_raw(
                                    self.0
                                        as *const crate::plugins::ox_shared::registry::Entry,
                                ))
                            };
                        }
                    }
                }
                let _retain_release = RetainGuard(retained);
                match rc {
                    x if x == ffi::OXPHP_SHARED_INVOKE_OK => {
                        let bytes = unsafe { std::slice::from_raw_parts(out_buf, out_len) };
                        let sv =
                            portbuf_to_sv(bytes).and_then(|raw| raw_to_owned(raw, entry.registry));
                        unsafe { ffi::oxphp_portable_free(out_buf) };
                        // The callable was valid and ran; a decode error here
                        // means the *returned value* is not shareable.
                        sv.map_err(|_| {
                            set_last_error(
                                "Once::getOrInit: factory returned a non-serialisable value \
                                 (closure / resource / non-Shareable object)",
                            );
                            OnceFactoryError::NotSerialisable
                        })
                    }
                    x if x == ffi::OXPHP_SHARED_INVOKE_PHP_THREW => {
                        if !out_buf.is_null() {
                            unsafe { ffi::oxphp_portable_free(out_buf) };
                        }
                        // Capture (class, message, code) WITHOUT clearing —
                        // the original exception still propagates to this caller.
                        Err(OnceFactoryError::Threw(capture_pending_exception()))
                    }
                    x if x == ffi::OXPHP_SHARED_INVOKE_BAD_RETURN => {
                        if !out_buf.is_null() {
                            unsafe { ffi::oxphp_portable_free(out_buf) };
                        }
                        // The callable ran to completion; the C-side
                        // portbuf serialiser rejected its return value
                        // (closure, resource, non-Shareable object).
                        // Distinguish from `Invalid` (the callable
                        // itself was unusable) so the surfaced message
                        // points at the right line of PHP.
                        set_last_error(
                            "Once::getOrInit: factory returned a non-serialisable value \
                             (closure / resource / non-Shareable object)",
                        );
                        Err(OnceFactoryError::NotSerialisable)
                    }
                    _ => {
                        if !out_buf.is_null() {
                            unsafe { ffi::oxphp_portable_free(out_buf) };
                        }
                        set_last_error("Once::getOrInit: factory is not a valid callable");
                        Err(OnceFactoryError::Invalid)
                    }
                }
            });

            // Count the access regardless of outcome (cached / ran / failed).
            entry.registry.record_op(entry);
            match result {
                Ok(v) => {
                    write_value_to_retval(call, &v)?;
                    Ok(())
                }
                Err(SharedError::Generic) => {
                    // PHP exception already pending; framework surfaces it.
                    Err(PhpError::Custom("Once::getOrInit factory threw".into()))
                }
                Err(SharedError::Poisoned) => Err(poisoned_error(inner)),
                Err(SharedError::Type) => {
                    // The factory closure paths above (NotSerialisable,
                    // Invalid) write a precise diagnostic via
                    // `set_last_error` ("factory returned a non-serialisable
                    // value …" / "factory is not a valid callable"). Surface
                    // it as the TypeException message instead of the generic
                    // `SharedError::Type` Display ("type error"); empty
                    // last_error falls back to the Display so the path stays
                    // safe under future callers that raise Type without
                    // writing last_error.
                    let detail = read_last_error_message();
                    let message = if detail.is_empty() {
                        SharedError::Type.to_string()
                    } else {
                        detail
                    };
                    Err(PhpError::Exception {
                        class: "OxPHP\\Shared\\TypeException".to_string(),
                        message,
                        code: 0,
                    })
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
            let id =
                unsafe { crate::plugins::ox_shared::registry::oxphp_shared_entry_id(h.entry_ptr) };
            call.ret_long(id as i64);
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
        SharedError::Timeout => "OxPHP\\Shared\\OperationTimeoutException",
        SharedError::Poisoned => "OxPHP\\Shared\\PoisonedException",
        SharedError::Closed => "OxPHP\\Shared\\ClosedException",
        SharedError::Cycle => "OxPHP\\Shared\\CycleException",
        _ => "OxPHP\\Shared\\SharedException",
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

/// Read a PHP argument as an owned `SharedValue`. Scalars take a direct
/// fast-path; arrays and nested Shared go through the portbuf codec (the
/// same path `getOrInit`'s factory uses), so `trySet` accepts the full
/// value range. Closures / resources are not serialisable and raise a
/// `TypeException`.
fn read_arg_as_shared_value(
    call: &mut crate::bridge::call::NativeCall,
    idx: u32,
    reg: &SharedRegistry,
) -> Result<SharedValue, PhpError> {
    use crate::bridge::ffi as bridge_ffi;
    use crate::plugins::ox_shared::value::{portbuf_to_sv, raw_to_owned};

    let t = call.arg_type(idx)?;
    match t {
        ValType::Null => return Ok(SharedValue::Null),
        ValType::True => return Ok(SharedValue::Bool(true)),
        ValType::False => return Ok(SharedValue::Bool(false)),
        ValType::Long => return Ok(SharedValue::Long(call.arg_long(idx)?)),
        ValType::Double => return Ok(SharedValue::Double(call.arg_double(idx)?)),
        ValType::String => return Ok(SharedValue::String(Arc::from(call.arg_str(idx)?))),
        _ => {}
    }

    // Arrays / nested Shared: serialise the zval to a portbuf, then decode.
    let arg_ptr = unsafe { call.raw_arg_ptr(idx) };
    let mut buf: *mut u8 = std::ptr::null_mut();
    let mut len: usize = 0;
    let rc =
        unsafe { bridge_ffi::oxphp_portable_serialize(arg_ptr as *const _, 1, &mut buf, &mut len) };
    if rc != 0 {
        if !buf.is_null() {
            unsafe { bridge_ffi::oxphp_portable_free(buf) };
        }
        return Err(type_error(
            "Once value is not serialisable (closure/resource)",
        ));
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf, len) };
    let sv = portbuf_to_sv(bytes).and_then(|raw| raw_to_owned(raw, reg));
    unsafe { bridge_ffi::oxphp_portable_free(buf) };
    sv.map_err(|e| err_to_phperr(e, 0))
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
        // Arrays and nested Shared go through the portbuf serialiser — the
        // same wire path Map::get uses — so a memoised config array round-trips.
        other => {
            use crate::bridge::ffi as bridge_ffi;
            use crate::plugins::ox_shared::value::sv_to_portbuf;
            let bytes = sv_to_portbuf(other);
            let rc = unsafe {
                bridge_ffi::oxphp_portable_deserialize(
                    bytes.as_ptr(),
                    bytes.len(),
                    1,
                    call.retval_ptr(),
                )
            };
            if rc != 0 {
                return Err(type_error("Once: failed to materialise stored value"));
            }
        }
    }
    Ok(())
}

/// Build a `PoisonedException` PhpError from the cell's captured info.
/// Carries the original factory exception's class+message (in the text) and
/// its numeric code (as the PoisonedException code), so a consumer reading
/// `$e->getCode()` sees the original code — matching the documented contract.
fn poisoned_error(inner: &OnceInner) -> PhpError {
    let (message, code) = match inner.poison_info() {
        Some(p) => (
            format!(
                "Once poisoned by failed factory: {}: {}",
                p.class, p.message
            ),
            p.code,
        ),
        None => ("Once poisoned by failed factory".to_string(), 0),
    };
    PhpError::Exception {
        class: "OxPHP\\Shared\\PoisonedException".to_string(),
        message,
        code,
    }
}

/// Copy the currently pending PHP exception's (class, message, code) into
/// owned strings. Does NOT clear it — the current caller still receives it.
fn capture_pending_exception() -> PoisonInfo {
    use crate::bridge::ffi as bridge_ffi;
    use std::ffi::CStr;
    let mut class_p: *const std::os::raw::c_char = std::ptr::null();
    let mut msg_p: *const std::os::raw::c_char = std::ptr::null();
    let mut code: i64 = 0;
    unsafe {
        if bridge_ffi::oxphp_exception_pending() != 0 {
            bridge_ffi::oxphp_exception_get(&mut class_p, &mut msg_p, &mut code);
        }
    }
    let to_string = |p: *const std::os::raw::c_char| -> String {
        if p.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    };
    PoisonInfo {
        class: to_string(class_p),
        message: to_string(msg_p),
        code,
    }
}

/// Emit a `Once\Status` enum-case singleton into the retval slot.
fn emit_status_case(
    call: &mut crate::bridge::call::NativeCall,
    case: &str,
) -> Result<(), PhpError> {
    use crate::bridge::ffi as bridge_ffi;
    const FQN: &str = "OxPHP\\Shared\\Once\\Status";
    let retval = call.retval_ptr();
    let rc = unsafe {
        bridge_ffi::oxphp_bridge_get_enum_case(
            retval,
            FQN.as_ptr() as *const _,
            FQN.len(),
            case.as_ptr() as *const _,
            case.len(),
        )
    };
    if rc != 0 {
        return Err(PhpError::Custom(format!(
            "failed to resolve enum case {FQN}::{case}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::ox_shared::value::SharedValue;

    fn pi() -> PoisonInfo {
        PoisonInfo {
            class: "E".into(),
            message: "boom".into(),
            code: 7,
        }
    }

    #[test]
    fn starts_uninitialized() {
        let o = OnceInner::new();
        assert_eq!(o.state(), ST_UNINIT);
        assert!(o.get().is_none());
        assert!(o.poison_info().is_none());
    }

    #[test]
    fn try_set_first_wins_then_ready() {
        let o = OnceInner::new();
        assert_eq!(o.try_set(SharedValue::Long(42)), Ok(true));
        assert_eq!(o.state(), ST_READY);
        assert_eq!(o.try_set(SharedValue::Long(99)), Ok(false));
        assert!(matches!(o.get(), Some(SharedValue::Long(42))));
    }

    #[test]
    fn get_or_init_runs_factory_once_and_caches() {
        let o = OnceInner::new();
        let v = o.get_or_init(|| Ok(SharedValue::Long(5))).expect("ok");
        assert!(matches!(v, SharedValue::Long(5)));
        // Second call must not run the factory.
        let v2 = o
            .get_or_init(|| panic!("factory must not run again"))
            .expect("cached");
        assert!(matches!(v2, SharedValue::Long(5)));
        assert_eq!(o.state(), ST_READY);
    }

    #[test]
    fn get_or_init_reset_mode_retries_after_throw() {
        let o = OnceInner::new(); // Reset
        let err = o
            .get_or_init(|| Err(OnceFactoryError::Threw(pi())))
            .unwrap_err();
        assert_eq!(err, SharedError::Generic);
        assert_eq!(o.state(), ST_UNINIT); // reset, retryable
        let v = o
            .get_or_init(|| Ok(SharedValue::Long(1)))
            .expect("retry ok");
        assert!(matches!(v, SharedValue::Long(1)));
    }

    #[test]
    fn get_or_init_poison_mode_poisons_terminally() {
        let o = OnceInner::with_mode(OnceFailureMode::Poison);
        let _ = o
            .get_or_init(|| Err(OnceFactoryError::Threw(pi())))
            .unwrap_err();
        assert_eq!(o.state(), ST_POISONED);
        let info = o.poison_info().expect("poison captured");
        assert_eq!(info.message, "boom");
        // Every value path now reports Poisoned.
        assert_eq!(o.try_set(SharedValue::Long(1)), Err(SharedError::Poisoned));
        assert_eq!(
            o.get_or_init(|| Ok(SharedValue::Long(1))).unwrap_err(),
            SharedError::Poisoned
        );
        // get() returns None (handler turns Poisoned-state into PoisonedException).
        assert!(o.get().is_none());
    }

    #[test]
    fn stored_null_is_ready_not_uninit() {
        let o = OnceInner::new();
        assert_eq!(o.try_set(SharedValue::Null), Ok(true));
        assert_eq!(o.state(), ST_READY);
        assert!(matches!(o.get(), Some(SharedValue::Null)));
    }

    #[test]
    fn poisoned_error_carries_original_code_and_message() {
        let o = OnceInner::with_mode(OnceFailureMode::Poison);
        let _ = o
            .get_or_init(|| Err(OnceFactoryError::Threw(pi())))
            .unwrap_err();
        match poisoned_error(&o) {
            PhpError::Exception {
                class,
                message,
                code,
            } => {
                assert_eq!(class, "OxPHP\\Shared\\PoisonedException");
                assert_eq!(code, 7, "original factory exception code must propagate");
                assert!(message.contains("boom"), "message: {message}");
            }
            other => panic!("expected Exception, got {other:?}"),
        }
    }

    #[test]
    fn factory_panic_resets_to_uninitialized_not_pending() {
        use std::panic::{catch_unwind, AssertUnwindSafe};
        let o = OnceInner::new();
        // A panic inside the factory (not an Err) must not strand the cell
        // in Pending — the RAII guard resets it to Uninitialized.
        let r = catch_unwind(AssertUnwindSafe(|| {
            o.get_or_init(|| panic!("boom in factory"))
        }));
        assert!(r.is_err(), "panic should propagate to the caller");
        assert_eq!(o.state(), ST_UNINIT, "cell must be retryable, not Pending");
        // And the cell is genuinely retryable afterwards.
        let v = o
            .get_or_init(|| Ok(SharedValue::Long(1)))
            .expect("retry after panic");
        assert!(matches!(v, SharedValue::Long(1)));
    }

    #[test]
    fn non_serialisable_return_is_distinct_from_invalid_callable() {
        // Both map to SharedError::Type (TypeException) and leave the cell
        // retryable under Reset mode, but the variants are distinct for
        // diagnostics.
        let o = OnceInner::new();
        assert_eq!(
            o.get_or_init(|| Err(OnceFactoryError::NotSerialisable))
                .unwrap_err(),
            SharedError::Type
        );
        assert_eq!(o.state(), ST_UNINIT);
        assert_eq!(
            o.get_or_init(|| Err(OnceFactoryError::Invalid))
                .unwrap_err(),
            SharedError::Type
        );
        assert_eq!(o.state(), ST_UNINIT);
    }

    #[test]
    fn non_serialisable_return_under_poison_mode_poisons_terminally() {
        // A factory that ran to completion but produced a non-Shareable
        // value is a factory failure in the same operational sense as
        // Threw — honour FailureMode::Poison.
        let o = OnceInner::with_mode(OnceFailureMode::Poison);
        assert_eq!(
            o.get_or_init(|| Err(OnceFactoryError::NotSerialisable))
                .unwrap_err(),
            SharedError::Type
        );
        assert_eq!(o.state(), ST_POISONED);
        let info = o.poison_info().expect("poison captured");
        assert_eq!(info.class, "OxPHP\\Shared\\TypeException");
        assert!(
            info.message.contains("non-serialisable"),
            "message: {}",
            info.message
        );
        // Cell is terminal: further calls report Poisoned, not Type.
        assert_eq!(
            o.get_or_init(|| Ok(SharedValue::Long(1))).unwrap_err(),
            SharedError::Poisoned
        );
        assert_eq!(o.try_set(SharedValue::Long(1)), Err(SharedError::Poisoned));
    }

    #[test]
    fn invalid_callable_under_poison_mode_still_resets() {
        // `Invalid` is a programmer mistake at the call site (the arg
        // was not callable at all) — a follow-up call with a real
        // callable must succeed regardless of FailureMode.
        let o = OnceInner::with_mode(OnceFailureMode::Poison);
        assert_eq!(
            o.get_or_init(|| Err(OnceFactoryError::Invalid))
                .unwrap_err(),
            SharedError::Type
        );
        assert_eq!(o.state(), ST_UNINIT);
        let v = o
            .get_or_init(|| Ok(SharedValue::Long(42)))
            .expect("retry after Invalid must succeed even under Poison");
        assert!(matches!(v, SharedValue::Long(42)));
    }
}
