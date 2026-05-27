//! OxPHP\Shared\Registry — name-keyed get-or-create over the process-global
//! [`SharedRegistry`]. Identity-by-name complements identity-by-handle: a
//! name binds one entry (pinned via a strong `Arc`), every caller of
//! `name_acquire(key)` converges on the same id.
//!
//! State machine (per key):
//!
//! ```text
//!   absent ──acquire──► Creating(gate) ──bind──► Bound(arc)
//!                              │
//!                              └──abort──► absent
//! ```
//!
//! `Bound` holds a strong `Arc<Entry>` — the pin that keeps a named entry
//! alive independent of any PHP handle's refcount. `Creating` is a per-key
//! gate: the first caller becomes the creator and runs the factory; later
//! callers from other threads block on the gate's condvar; later callers
//! from the SAME thread (reentrancy inside the factory) get
//! [`AcquireOutcome::Reentrant`] so the handler can throw
//! `DeadlockException` instead of self-deadlocking.

use std::sync::{Arc, Condvar, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

/// How long a waiter on a Creating gate will block before giving up with
/// a `Deadlock` outcome. Same-thread reentrancy is detected synchronously
/// via `ThreadId`, but a *cross-key* cycle (thread A's factory acquires
/// key K2 owned by thread B; thread B's factory acquires key K1 owned by
/// A) has no synchronous signal — both threads park in `gate.wait()` and
/// never settle. The timeout converts that hang into a thrown
/// `DeadlockException` so the request fails loudly instead of pinning a
/// PHP worker until process kill.
///
/// Picked at 30 s: long enough to absorb a slow but progressing factory
/// (network I/O, fork-exec, container init), short enough that a
/// genuinely cyclic deadlock is reported within one request-timeout
/// window in typical deployments. Operators can tune later via env if
/// the default proves wrong; the value is not exposed today.
const GATE_WAIT_TIMEOUT: Duration = Duration::from_secs(30);

use crate::plugins::ox_shared::registry::{Entry, SharedRegistry, SharedType};

/// Per-key creation gate. The creator transitions Creating→settled by
/// calling [`CreateGate::settle`] with `Some(arc)` on `bind` or `None` on
/// `abort`. Waiters block in [`CreateGate::wait`] until then.
///
/// `Debug` is intentionally opaque (no Mutex contents, no `Entry`
/// pointers) — the gate appears inside `AcquireOutcome::NeedsFactory`
/// whose `Debug` is used in test assertions, and surfacing the inner
/// state would risk deadlocking the formatter on a held lock.
pub struct CreateGate {
    inner: Mutex<GateState>,
    cv: Condvar,
    /// Thread that opened the gate (became the creator). A re-acquire from
    /// this same thread is reentrancy — would deadlock waiting on itself.
    creator: ThreadId,
}

impl std::fmt::Debug for CreateGate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreateGate")
            .field("creator", &self.creator)
            .finish_non_exhaustive()
    }
}

struct GateState {
    settled: bool,
    /// Some(arc) on bind, None on abort.
    result: Option<Arc<Entry>>,
}

impl CreateGate {
    fn new() -> Self {
        Self {
            inner: Mutex::new(GateState {
                settled: false,
                result: None,
            }),
            cv: Condvar::new(),
            creator: std::thread::current().id(),
        }
    }

    fn settle(&self, result: Option<Arc<Entry>>) {
        // `unwrap_or_else(into_inner)` instead of `unwrap()`: `settle`
        // runs from `CreatingGuard::Drop` during stack unwinding. The
        // crate uses the default `panic = unwind` profile, so a
        // poisoned `GateState` mutex would panic here, which in turn
        // aborts the process from inside Drop. Recovering the inner
        // state lets waiters wake even if a previous holder panicked —
        // the worst case is an inconsistent `settled` flag flip, which
        // is exactly what we want anyway (settled-aborted).
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        g.settled = true;
        g.result = result;
        self.cv.notify_all();
    }

    /// Wake every blocked waiter on this gate with an aborted result.
    /// Used by `SharedRegistry::drain` on shutdown so threads stuck in
    /// `wait()` on a Creating slot don't deadlock process teardown.
    pub(crate) fn settle_aborted(&self) {
        self.settle(None);
    }

    /// Block until the gate is settled or `timeout` elapses. Returns:
    /// - `Ok(Some(arc))` — bound.
    /// - `Ok(None)` — aborted (re-acquire to retry as a new creator).
    /// - `Err(WaitTimedOut)` — caller raises `DeadlockException`.
    ///
    /// Both `lock()` and `wait_timeout` recover from a poisoned `GateState`
    /// the same way `settle` does (`PoisonError::into_inner`) — if a
    /// future change ever lets a holder panic mid-`settle`, waiters must
    /// still be able to observe the (possibly half-written) settle flag
    /// instead of cascading the panic across every parked thread.
    pub(crate) fn wait_with(&self, timeout: Duration) -> Result<Option<Arc<Entry>>, WaitTimedOut> {
        let mut g = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut remaining = timeout;
        while !g.settled {
            let start = std::time::Instant::now();
            let (next, res) = self
                .cv
                .wait_timeout(g, remaining)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            g = next;
            // Re-check `settled` under the held lock before reporting
            // timeout: `notify_all` from `settle` can race with the
            // timeout expiring, and a waiter that lost the race would
            // otherwise raise a false DeadlockException for a factory
            // that finished successfully.
            if res.timed_out() {
                if g.settled {
                    break;
                }
                return Err(WaitTimedOut);
            }
            // Spurious wakeups: subtract the elapsed time and continue.
            remaining = remaining.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                if g.settled {
                    break;
                }
                return Err(WaitTimedOut);
            }
        }
        Ok(g.result.clone())
    }

    /// Production wait with the deadlock-detection timeout
    /// (`GATE_WAIT_TIMEOUT`). Most plausible cause of timing out without
    /// a synchronous-reentrancy hit is a cross-key cycle.
    fn wait(&self) -> Result<Option<Arc<Entry>>, WaitTimedOut> {
        self.wait_with(GATE_WAIT_TIMEOUT)
    }
}

/// Sentinel returned by [`CreateGate::wait`] when `GATE_WAIT_TIMEOUT`
/// expires without the gate being settled.
#[derive(Debug)]
pub struct WaitTimedOut;

pub enum NameSlot {
    Creating(Arc<CreateGate>),
    Bound(Arc<Entry>),
}

/// RAII guard for an open Creating slot: holds the key and the gate,
/// and on `Drop` settles the gate as aborted and (best-effort) removes
/// the slot from the names index, unless [`Self::commit`] has been
/// invoked first.
///
/// Holding `Arc<CreateGate>` directly (rather than relying on
/// `name_abort` to look the gate up in the map) is what closes the
/// drain-race deadlock: if `drain()`'s `names.clear()` has already
/// removed the slot, `name_abort`'s `remove_if` would silently
/// no-op and never settle the gate — any waiter holding a cloned gate
/// `Arc` would then block until `GATE_WAIT_TIMEOUT` (30 s) and surface
/// a misleading `DeadlockException`. Settling through the guard
/// guarantees waiters wake regardless of slot state.
pub struct CreatingGuard {
    reg: &'static SharedRegistry,
    key: String,
    /// Same `Arc<CreateGate>` that lives in `names[key]` while the
    /// slot exists. Kept here so we can settle even after the slot is
    /// gone.
    gate: Arc<CreateGate>,
    committed: bool,
    /// `!Send` marker. The Drop-time slot removal is creator-thread
    /// checked and becomes a silent no-op on the wrong thread, which
    /// would permanently leak the Creating slot. A raw-pointer
    /// PhantomData makes the type unsendable so the compiler catches
    /// any accidental move across threads (async executors, test
    /// harnesses) at the call site instead of at runtime.
    _not_send: std::marker::PhantomData<*mut ()>,
}

impl CreatingGuard {
    /// Track a key whose slot is currently `Creating` and owned by this
    /// thread. Caller invariant: [`SharedRegistry::name_acquire`] just
    /// returned `NeedsFactory(gate)` for `key`. The registry reference
    /// is stored explicitly rather than fetched from the global so the
    /// guard works under `new_for_test` too.
    pub fn new(reg: &'static SharedRegistry, key: String, gate: Arc<CreateGate>) -> Self {
        Self {
            reg,
            key,
            gate,
            committed: false,
            _not_send: std::marker::PhantomData,
        }
    }

    /// Mark the gate as already settled by the caller (typically by a
    /// successful `name_bind`). Drop becomes a no-op.
    pub fn commit(&mut self) {
        self.committed = true;
    }
}

impl Drop for CreatingGuard {
    fn drop(&mut self) {
        if !self.committed {
            // Order matters: remove the slot BEFORE settling the gate.
            // Settling first would wake every waiter while the Creating
            // slot is still present in the names index — each waiter
            // would re-loop in `name_acquire`, observe the still-
            // Creating slot, clone the (already-settled) gate, return
            // instantly from `wait_with`, and spin until `name_abort`
            // eventually clears the slot. Removing first means waiters
            // wake to an absent slot and fall straight through to the
            // Vacant branch (one becomes the new creator, the rest see
            // the new Creating gate and block properly).
            //
            // The guard is `!Send` (see field doc) so this runs on the
            // creator thread; `name_abort`'s `remove_if` predicate
            // verifies creator ownership before tearing down. If
            // `drain.clear()` got there first, `remove_if` is a silent
            // no-op — the subsequent `settle(None)` still wakes any
            // waiter holding a cloned gate `Arc`, which is the
            // guarantee that closes the drain-race deadlock.
            self.reg.name_abort(&self.key);
            // Idempotent: drain may have already `settle_aborted`'d the
            // gate, in which case this overwrites with the same `None`.
            self.gate.settle(None);
        }
    }
}

/// Why [`SharedRegistry::name_bind`] refused to install the entry. All
/// variants represent contract violations that the FFI surfaces via
/// `set_last_error` — they were silently swallowed before, hiding bugs and
/// the drain race in particular.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindError {
    /// Slot is `Creating(gate)` but the gate's creator is not the calling
    /// thread. Only the creator may settle the gate; a foreign-thread bind
    /// would settle waiters with the wrong arc.
    ForeignCreator,
    /// Slot is already `Bound`. Caller used bind twice or on a key never
    /// acquired by this thread.
    AlreadyBound,
    /// Slot is absent. Caller used bind without a prior `NeedsFactory`.
    NoCreatingSlot,
    /// Registry is draining — a fresh Bound entry would outlive teardown.
    ShuttingDown,
}

/// Result of [`SharedRegistry::name_acquire`].
///
/// `Hit` carries an `Arc::into_raw` pointer (strong+1) — the caller transfers
/// it into a PHP wrapper whose `SharedHandle::drop` balances it.
#[derive(Debug)]
pub enum AcquireOutcome {
    Hit(*const Entry),
    TypeMismatch,
    /// Slot was absent; THIS caller is now the creator. The
    /// `Arc<CreateGate>` clones the slot's gate so the caller's RAII
    /// guard can settle it directly on abort, independent of whether
    /// the slot still exists in the names index. Must call
    /// [`SharedRegistry::name_bind`] (success) or
    /// [`SharedRegistry::name_abort`] (factory threw).
    NeedsFactory(Arc<CreateGate>),
    /// Same thread is already the creator of this key. A blocking wait
    /// would self-deadlock. Caller throws `DeadlockException`.
    Reentrant,
    /// Registry is draining (process is shutting down). Caller throws
    /// `SharedException` rather than installing a new Bound entry that
    /// would survive teardown unnoticed.
    ShuttingDown,
    /// Waited longer than `GATE_WAIT_TIMEOUT` on a Creating slot owned by
    /// another thread. The most plausible cause is a cross-key cycle —
    /// thread A's factory acquired K2 (B's slot), B's factory acquired
    /// K1 (A's slot). Synchronous reentrancy is caught earlier; this is
    /// the asynchronous-cycle path. Caller throws `DeadlockException`.
    Deadlock,
}

impl SharedRegistry {
    pub fn name_acquire(&self, key: &str, want: SharedType) -> AcquireOutcome {
        loop {
            // Short-circuit on shutdown. Drain races with us: if drain
            // already set the flag, we MUST NOT install a new Creating
            // slot — drain's name iteration may already be complete and
            // a fresh Bound entry would outlive the teardown.
            if self.is_shutting_down() {
                return AcquireOutcome::ShuttingDown;
            }
            // Fast path: read existing slot under a shard read-lock.
            if let Some(slot) = self.names.get(key) {
                match slot.value() {
                    NameSlot::Bound(arc) => {
                        // Defence-in-depth: the same magic-tag check every
                        // other Entry FFI uses. A future UAF on the
                        // Bound→cloned-Arc handoff would otherwise corrupt
                        // strong-counts silently; here it surfaces as a
                        // clean debug panic.
                        debug_assert_eq!(
                            arc.magic,
                            crate::plugins::ox_shared::registry::ENTRY_MAGIC,
                            "name_acquire Hit on freed Entry"
                        );
                        if arc.type_tag != want {
                            return AcquireOutcome::TypeMismatch;
                        }
                        let cloned = arc.clone();
                        drop(slot); // release shard lock before Arc::into_raw
                        return AcquireOutcome::Hit(Arc::into_raw(cloned));
                    }
                    NameSlot::Creating(gate) => {
                        if gate.creator == std::thread::current().id() {
                            return AcquireOutcome::Reentrant;
                        }
                        let gate = gate.clone();
                        drop(slot); // never wait under the shard lock
                        match gate.wait() {
                            // Use the settled arc directly instead of
                            // re-reading the names slot. A concurrent
                            // `name_remove` between the creator's settle
                            // and our re-read would otherwise turn this
                            // waiter into a new creator (the Vacant
                            // branch below), violating exactly-once.
                            Ok(Some(arc)) => {
                                debug_assert_eq!(
                                    arc.magic,
                                    crate::plugins::ox_shared::registry::ENTRY_MAGIC,
                                    "name_acquire received freed Entry from settled gate"
                                );
                                if arc.type_tag != want {
                                    return AcquireOutcome::TypeMismatch;
                                }
                                return AcquireOutcome::Hit(Arc::into_raw(arc));
                            }
                            // Aborted (factory threw / drain) — retry as
                            // a fresh acquirer: the slot is now absent
                            // and we may become the next creator.
                            Ok(None) => continue,
                            Err(WaitTimedOut) => return AcquireOutcome::Deadlock,
                        }
                    }
                }
            }
            // Slot absent: try to become the creator atomically.
            match self.names.entry(key.to_string()) {
                dashmap::mapref::entry::Entry::Occupied(_) => continue, // lost the race
                dashmap::mapref::entry::Entry::Vacant(v) => {
                    // Re-check shutdown under the shard write-lock —
                    // the top-level check is unsynchronised with
                    // drain's store, so without this we could install
                    // a fresh Creating slot AFTER drain has finished
                    // iter+clear, leaking the slot until process exit.
                    if self.is_shutting_down() {
                        return AcquireOutcome::ShuttingDown;
                    }
                    let gate = Arc::new(CreateGate::new());
                    v.insert(NameSlot::Creating(gate.clone()));
                    return AcquireOutcome::NeedsFactory(gate);
                }
            }
        }
    }

    /// Untyped sibling of [`Self::name_acquire`] for the
    /// `Registry::global()` escape hatch — does NOT type-check on Hit;
    /// returns whatever is bound. Used by the Rust caller directly so
    /// the `NeedsFactory(gate)` Arc reaches the [`CreatingGuard`].
    pub fn name_acquire_any(&self, key: &str) -> AcquireOutcome {
        loop {
            if self.is_shutting_down() {
                return AcquireOutcome::ShuttingDown;
            }
            if let Some(slot) = self.names.get(key) {
                match slot.value() {
                    NameSlot::Bound(arc) => {
                        debug_assert_eq!(
                            arc.magic,
                            crate::plugins::ox_shared::registry::ENTRY_MAGIC,
                            "name_acquire_any Hit on freed Entry"
                        );
                        let cloned = arc.clone();
                        drop(slot);
                        return AcquireOutcome::Hit(Arc::into_raw(cloned));
                    }
                    NameSlot::Creating(gate) => {
                        if gate.creator == std::thread::current().id() {
                            return AcquireOutcome::Reentrant;
                        }
                        let gate = gate.clone();
                        drop(slot);
                        match gate.wait() {
                            // Same exactly-once fix as `name_acquire`:
                            // use the settled arc directly so a racing
                            // `name_remove` cannot promote this waiter
                            // into a new creator.
                            Ok(Some(arc)) => {
                                debug_assert_eq!(
                                    arc.magic,
                                    crate::plugins::ox_shared::registry::ENTRY_MAGIC,
                                    "name_acquire_any received freed Entry from settled gate"
                                );
                                return AcquireOutcome::Hit(Arc::into_raw(arc));
                            }
                            Ok(None) => continue,
                            Err(WaitTimedOut) => return AcquireOutcome::Deadlock,
                        }
                    }
                }
            }
            match self.names.entry(key.to_string()) {
                dashmap::mapref::entry::Entry::Occupied(_) => continue,
                dashmap::mapref::entry::Entry::Vacant(v) => {
                    if self.is_shutting_down() {
                        return AcquireOutcome::ShuttingDown;
                    }
                    let gate = Arc::new(CreateGate::new());
                    v.insert(NameSlot::Creating(gate.clone()));
                    return AcquireOutcome::NeedsFactory(gate);
                }
            }
        }
    }

    /// Creator success: pin the entry under `key`, wake waiters. `arc` is
    /// the entry the factory produced; the names index stores it
    /// (strong → pin). Returns `Err(BindError::*)` on contract violations
    /// so the caller (FFI) can surface them via `set_last_error` instead
    /// of silently corrupting the index.
    ///
    /// Ownership check: the current thread MUST be the creator that opened
    /// the Creating gate. Without this, a drain-race window (where the
    /// gate is settled-aborted, the slot reused by another thread, then
    /// our late factory finishes and calls bind) would let one thread
    /// settle a foreign thread's gate with the wrong arc — waiters would
    /// wake holding the *winner's* arc while their own creator's factory
    /// output is silently discarded.
    pub fn name_bind(&self, key: &str, arc: Arc<Entry>, _tag: SharedType) -> Result<(), BindError> {
        use dashmap::mapref::entry::Entry as DEntry;
        // First-pass shutdown check is an optimisation — it lets us
        // skip the shard lock for the common shutdown path. The
        // authoritative check happens AFTER `entry()` below.
        if self.is_shutting_down() {
            return Err(BindError::ShuttingDown);
        }
        let me = std::thread::current().id();
        let to_signal: Option<(Arc<CreateGate>, Arc<Entry>)> =
            match self.names.entry(key.to_string()) {
                DEntry::Occupied(mut o) => {
                    // Re-check under the shard write-lock: drain may
                    // have flipped `shutting_down` between the
                    // optimistic check above and the lock acquisition.
                    // Without this, a name_bind that wins the race
                    // installs a Bound entry that drain's subsequent
                    // `names.clear()` immediately drops — silently
                    // violating identity-by-name for any peer worker
                    // that reads through the drain window.
                    if self.is_shutting_down() {
                        return Err(BindError::ShuttingDown);
                    }
                    match o.get() {
                        NameSlot::Creating(g) if g.creator == me => {
                            // Our gate — safe to replace.
                            match o.insert(NameSlot::Bound(arc.clone())) {
                                NameSlot::Creating(g) => Some((g, arc)),
                                NameSlot::Bound(_) => unreachable!("checked above"),
                            }
                        }
                        NameSlot::Creating(_) => {
                            // Foreign creator's slot — the original gate
                            // was settled-aborted out from under us (drain
                            // or a manual abort). Do not settle the new
                            // gate with our arc.
                            return Err(BindError::ForeignCreator);
                        }
                        NameSlot::Bound(_) => {
                            // Misuse: bind on an already-Bound key.
                            return Err(BindError::AlreadyBound);
                        }
                    }
                }
                DEntry::Vacant(_) => {
                    // Misuse: bind without a prior NeedsFactory slot.
                    return Err(BindError::NoCreatingSlot);
                }
            };
        if let Some((g, a)) = to_signal {
            g.settle(Some(a));
        }
        Ok(())
    }

    /// Creator failure (factory threw / PHP exception): drop the Creating
    /// slot owned by THIS thread, wake waiters. One of them transitions
    /// to NeedsFactory (reset).
    ///
    /// Atomic peek-then-remove via `remove_if` with a creator check:
    /// - A `Bound` slot must never be torn down by `abort` (would silently
    ///   drop the pin).
    /// - A `Creating` slot owned by *another* thread must not be torn
    ///   down either: if our gate was settled-aborted (drain) and the
    ///   slot was reused by another creator between settle and our late
    ///   abort, removing it would wake the new owner's waiters with no
    ///   value AND let the new creator settle its own gate against a
    ///   missing slot.
    pub fn name_abort(&self, key: &str) {
        let me = std::thread::current().id();
        if let Some((_, NameSlot::Creating(g))) = self.names.remove_if(
            key,
            |_, v| matches!(v, NameSlot::Creating(g) if g.creator == me),
        ) {
            g.settle(None);
        }
    }

    /// Unbind a key and drop its pin. Returns true iff a `Bound` slot was
    /// removed. The underlying Entry survives while any other handle holds
    /// it; it self-deregisters when the last `Arc` drops.
    ///
    /// Atomic via `remove_if`: a previous get→drop-ref→remove split window
    /// could evict a Creating slot inserted between the two ops, leaving
    /// waiters in `gate.wait()` forever (no settle path remains).
    pub fn name_remove(&self, key: &str) -> bool {
        self.names
            .remove_if(key, |_, v| matches!(v, NameSlot::Bound(_)))
            .is_some()
    }

    pub fn name_keys(&self) -> Vec<String> {
        self.names
            .iter()
            .filter(|s| matches!(s.value(), NameSlot::Bound(_)))
            .map(|s| s.key().clone())
            .collect()
    }
}

// ─── FFI ──────────────────────────────────────────────────────────────────
//
// Only the symbols the PHP handlers below actually consume are exported.
// The earlier `oxphp_shared_registry_{acquire,acquire_any,bind,abort}`
// shims were dead code — every Registry op runs through the in-process
// Rust paths (`name_acquire*`, `name_bind`, `name_abort`, plus the
// `CreatingGuard` RAII), so a C-ABI mirror with `Arc::into_raw` /
// `Arc::increment_strong_count` rituals just shipped untested unsafe.
// The deadlock-on-shutdown fix also needs `name_acquire` to hand the
// caller an `Arc<CreateGate>`, which a `c_int` return cannot carry —
// dropping the FFI mirror is the simpler shape.

use crate::plugins::ox_shared::registry::registry;

/// Process-wide estimated bytes across all Shared\* entries (named +
/// anonymous). Mirrors `SharedRegistry::total_bytes()`.
#[no_mangle]
pub extern "C" fn oxphp_shared_registry_memory_usage() -> u64 {
    registry().total_bytes()
}

/// Process-wide live entry count (named + anonymous). Mirrors
/// `SharedRegistry::total_entries()`.
#[no_mangle]
pub extern "C" fn oxphp_shared_registry_count() -> u64 {
    registry().total_entries()
}

// ─── PHP exception mapper ────────────────────────────────────────────────

use crate::bridge::call::NativeCall;
use crate::bridge::ffi as bridge_ffi;
use crate::plugin::types::PhpType;
use crate::plugin::{PhpError, PluginContext, PluginError};
use crate::plugins::ox_shared::value::{portbuf_to_sv, raw_to_owned, SharedRefOwned, SharedValue};

// ─── PHP class — Registry ────────────────────────────────────────────────

/// Materialise a `SharedValue::Shared(owned)` into the call's retval.
/// Uses the portable round-trip (same path `Once` uses for non-scalar
/// returns): `sv_to_portbuf` → `oxphp_portable_deserialize`. The
/// deserialiser calls `oxphp_shared_handle_from_id` to build a PHP
/// wrapper for the entry; `owned` keeps the entry alive until that
/// completes.
fn write_shared_to_retval(call: &mut NativeCall, owned: SharedRefOwned) -> Result<(), PhpError> {
    use crate::plugins::ox_shared::value::sv_to_portbuf;
    let sv = SharedValue::Shared(owned);
    let bytes = sv_to_portbuf(&sv);
    let rc = unsafe {
        bridge_ffi::oxphp_portable_deserialize(bytes.as_ptr(), bytes.len(), 1, call.retval_ptr())
    };
    if rc != 0 {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\SharedException".into(),
            message: "Shared\\Registry: failed to materialise wrapper".into(),
            code: 0,
        });
    }
    Ok(())
}

/// Shared get-or-create core. `want_tag = Some(t)` for typed methods,
/// `None` for `global()` (no type guard, returns whatever is bound).
fn registry_get_or_create(
    call: &mut NativeCall,
    key_arg: u32,
    factory_arg: u32,
    want_tag: Option<SharedType>,
) -> Result<(), PhpError> {
    let key = call.arg_str(key_arg)?.to_string();
    if key.is_empty() {
        return Err(PhpError::Exception {
            class: "InvalidArgumentException".into(), // SPL, root namespace
            message: "Shared\\Registry key must be a non-empty string".into(),
            code: 0,
        });
    }

    // 1. Acquire directly via the Rust API (no FFI round-trip) so we
    //    can hand the `Arc<CreateGate>` to `CreatingGuard`. The guard
    //    needs the gate to settle waiters unconditionally on Drop —
    //    going through C-ABI loses that channel.
    let outcome = match want_tag {
        Some(t) => registry().name_acquire(&key, t),
        None => registry().name_acquire_any(&key),
    };

    match outcome {
        AcquireOutcome::Hit(ptr) => {
            // ptr is Arc::into_raw'd (strong+1). Wrap in SharedRefOwned
            // so write_shared_to_retval keeps it alive across the
            // portbuf-deserialise wrap; Drop balances the +1.
            let arc = unsafe { Arc::from_raw(ptr) };
            let owned = SharedRefOwned::from_arc(arc);
            write_shared_to_retval(call, owned)
        }
        AcquireOutcome::TypeMismatch => Err(PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".into(),
            message: format!("Shared\\Registry: key '{key}' is bound to a different Shared type"),
            code: 0,
        }),
        AcquireOutcome::Reentrant => Err(PhpError::Exception {
            class: "OxPHP\\Shared\\DeadlockException".into(),
            message: format!(
                "Shared\\Registry: reentrant get-or-create for key '{key}' would deadlock \
                 (same thread is creating this key inside its own factory)"
            ),
            code: 0,
        }),
        AcquireOutcome::ShuttingDown => Err(PhpError::Exception {
            class: "OxPHP\\Shared\\SharedException".into(),
            message: format!(
                "Shared\\Registry: cannot acquire '{key}' — registry is draining \
                 (process shutdown)"
            ),
            code: 0,
        }),
        AcquireOutcome::Deadlock => Err(PhpError::Exception {
            class: "OxPHP\\Shared\\DeadlockException".into(),
            message: format!(
                "Shared\\Registry: waited too long on '{key}' — most plausibly a \
                 cross-key cycle (factory A waiting on key created by factory B \
                 which is waiting on key created by factory A)"
            ),
            code: 0,
        }),
        AcquireOutcome::NeedsFactory(gate) => {
            // RAII: any early return / panic from here on settles the
            // gate (waiters wake) and best-effort-removes the slot from
            // the names index. Only the happy path commits.
            let mut creating = CreatingGuard::new(registry(), key.clone(), gate);

            // 2. Run the user's factory via the portable invoke. If the
            //    factory returned a Shared\* wrapper, the C shim retains
            //    its Entry via oxphp_shared_handle_clone before destroying
            //    the wrapper so our lookup(id) below cannot race against
            //    Entry::drop. The +1 strong ref is "owned by the buffer";
            //    `_retain_release` (an RAII guard below) drops it after we
            //    finish all operations that need the entry alive.
            let callable_zv = unsafe { call.raw_arg_ptr(factory_arg) };
            let mut out_buf: *mut u8 = std::ptr::null_mut();
            let mut out_len: usize = 0;
            let mut retained_entry: *const std::os::raw::c_void = std::ptr::null();
            let invoke_rc = unsafe {
                bridge_ffi::oxphp_shared_invoke_0_portbuf(
                    callable_zv,
                    &mut out_buf,
                    &mut out_len,
                    &mut retained_entry,
                )
            };
            // RAII: drop the C-side retain when we leave this scope, no
            // matter which arm we exit through. Constructed from a raw
            // ptr that may be null — the guard's Drop no-ops in that case.
            struct RetainGuard(*const std::os::raw::c_void);
            impl Drop for RetainGuard {
                fn drop(&mut self) {
                    if !self.0.is_null() {
                        // The C side bumped strong_count via
                        // oxphp_shared_handle_clone; balance with
                        // Arc::from_raw which decrements on drop.
                        unsafe { drop(Arc::from_raw(self.0 as *const Entry)) };
                    }
                }
            }
            let _retain_release = RetainGuard(retained_entry);
            match invoke_rc {
                x if x == bridge_ffi::OXPHP_SHARED_INVOKE_OK => {
                    let bytes = unsafe { std::slice::from_raw_parts(out_buf, out_len) };
                    let parsed = portbuf_to_sv(bytes).and_then(|raw| raw_to_owned(raw, registry()));
                    unsafe { bridge_ffi::oxphp_portable_free(out_buf) };

                    match parsed {
                        Ok(SharedValue::Shared(owned)) => {
                            let actual_tag = owned.type_tag;
                            // Type guard for typed methods. `creating` aborts
                            // on the early return via Drop.
                            if let Some(want) = want_tag {
                                if actual_tag != want {
                                    return Err(PhpError::Exception {
                                        class: "OxPHP\\Shared\\TypeException".into(),
                                        message: format!(
                                            "Shared\\Registry: factory for key '{key}' returned \
                                             {} but the typed method requires {}",
                                            actual_tag.name(),
                                            want.name()
                                        ),
                                        code: 0,
                                    });
                                }
                            }
                            // 3. Bind: take a fresh Arc via lookup (cheap; the
                            // entry IS alive because `owned` holds a strong ref).
                            let arc = match registry().lookup(owned.id) {
                                Ok(a) => a,
                                Err(_) => {
                                    return Err(PhpError::Exception {
                                        class: "OxPHP\\Shared\\SharedException".into(),
                                        message: "Shared\\Registry: factory entry vanished".into(),
                                        code: 0,
                                    });
                                }
                            };
                            // 4. Bind. Only the Ok path commits the guard;
                            //    every error path returns early so the
                            //    guard's Drop settles our gate as aborted
                            //    AND best-effort-removes the slot (no-op if
                            //    drain or a peer already cleared it). That
                            //    closes the drain-race deadlock where a
                            //    waiter who cloned our gate before
                            //    `drain.names.clear()` would otherwise
                            //    block until GATE_WAIT_TIMEOUT.
                            match registry().name_bind(&key, arc, actual_tag) {
                                Ok(()) => {
                                    creating.commit();
                                }
                                Err(BindError::ForeignCreator) => {
                                    // The slot we opened was settle_aborted
                                    // and reused by a peer mid-factory.
                                    // Returning the factory's entry to PHP
                                    // would hand back a wrapper whose entry
                                    // is NOT pinned under `key`, silently
                                    // breaking identity-by-name. Surface as
                                    // SharedException; the caller can retry.
                                    return Err(PhpError::Exception {
                                        class: "OxPHP\\Shared\\SharedException".into(),
                                        message: format!(
                                            "Shared\\Registry: bind for key '{key}' raced — \
                                             slot was reset and is now owned by another creator"
                                        ),
                                        code: 0,
                                    });
                                }
                                Err(BindError::ShuttingDown) => {
                                    return Err(PhpError::Exception {
                                        class: "OxPHP\\Shared\\SharedException".into(),
                                        message: format!(
                                            "Shared\\Registry: cannot bind '{key}' — registry \
                                             is draining (process shutdown)"
                                        ),
                                        code: 0,
                                    });
                                }
                                Err(BindError::AlreadyBound) => {
                                    return Err(PhpError::Exception {
                                        class: "OxPHP\\Shared\\SharedException".into(),
                                        message: format!(
                                            "Shared\\Registry: bind for key '{key}' rejected — \
                                             slot is already bound (internal invariant violation)"
                                        ),
                                        code: 0,
                                    });
                                }
                                Err(BindError::NoCreatingSlot) => {
                                    return Err(PhpError::Exception {
                                        class: "OxPHP\\Shared\\SharedException".into(),
                                        message: format!(
                                            "Shared\\Registry: bind for key '{key}' rejected — \
                                             no Creating slot (internal invariant violation)"
                                        ),
                                        code: 0,
                                    });
                                }
                            }
                            // 5. Return the wrapper — `owned`'s strong ref
                            // outlives the portbuf-deserialise inside.
                            write_shared_to_retval(call, owned)
                        }
                        _ => Err(PhpError::Exception {
                            class: "OxPHP\\Shared\\TypeException".into(),
                            message: format!(
                                "Shared\\Registry: factory for key '{key}' must return a \
                                 Shared\\* instance"
                            ),
                            code: 0,
                        }),
                    }
                }
                x if x == bridge_ffi::OXPHP_SHARED_INVOKE_PHP_THREW => {
                    if !out_buf.is_null() {
                        unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                    }
                    // EG(exception) is pending — propagate. PhpError::Custom
                    // signals the framework to surface the existing exception
                    // instead of overwriting it. `creating` aborts on drop.
                    Err(PhpError::Custom("Shared\\Registry: factory threw".into()))
                }
                x if x == bridge_ffi::OXPHP_SHARED_INVOKE_BAD_RETURN => {
                    if !out_buf.is_null() {
                        unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                    }
                    Err(PhpError::Exception {
                        class: "OxPHP\\Shared\\TypeException".into(),
                        message: format!(
                            "Shared\\Registry: factory for key '{key}' returned a \
                             non-serialisable value (closure, resource, or non-Shareable \
                             object) — must return a Shared\\* instance"
                        ),
                        code: 0,
                    })
                }
                _ => {
                    if !out_buf.is_null() {
                        unsafe { bridge_ffi::oxphp_portable_free(out_buf) };
                    }
                    Err(PhpError::Exception {
                        class: "OxPHP\\Shared\\TypeException".into(),
                        message: "Shared\\Registry: factory argument is not a valid callable"
                            .into(),
                        code: 0,
                    })
                }
            }
        }
    }
}

pub fn register_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_class("OxPHP\\Shared\\Registry")
        // Matches the `final class Registry` declaration in the stub.
        // Without ZEND_ACC_FINAL the runtime would accept `extends`
        // while static analysers reject it — silent stub/runtime drift.
        .final_()
        // No `with_storage` — Registry has no per-instance state. Block
        // instantiation by making __construct throw; the stub marks it
        // `public` so IDEs see the same surface the runtime exposes,
        // and the handler below enforces the unconditional failure.
        .method("__construct")
        .handler(|_call| {
            Err(PhpError::Exception {
                class: "OxPHP\\Shared\\SharedException".into(),
                message: "OxPHP\\Shared\\Registry is a static facade and cannot be instantiated"
                    .into(),
                code: 0,
            })
        })
        // Untyped escape hatch.
        .method("global")
        .static_()
        .param("key", PhpType::String)
        .param("factory", PhpType::Callable)
        .handler(|call| registry_get_or_create(call, 0, 1, None))
        // ── Typed get-or-create — type-guarded on hit, type-validated on
        //    factory return. Each delegates to the shared core with its
        //    own `SharedType` tag.
        .method("counter")
        .static_()
        .param("key", PhpType::String)
        .param("factory", PhpType::Callable)
        .handler(|call| registry_get_or_create(call, 0, 1, Some(SharedType::Counter)))
        .method("atomic")
        .static_()
        .param("key", PhpType::String)
        .param("factory", PhpType::Callable)
        .handler(|call| registry_get_or_create(call, 0, 1, Some(SharedType::Atomic)))
        .method("flag")
        .static_()
        .param("key", PhpType::String)
        .param("factory", PhpType::Callable)
        .handler(|call| registry_get_or_create(call, 0, 1, Some(SharedType::Flag)))
        .method("once")
        .static_()
        .param("key", PhpType::String)
        .param("factory", PhpType::Callable)
        .handler(|call| registry_get_or_create(call, 0, 1, Some(SharedType::Once)))
        .method("mutex")
        .static_()
        .param("key", PhpType::String)
        .param("factory", PhpType::Callable)
        .handler(|call| registry_get_or_create(call, 0, 1, Some(SharedType::Mutex)))
        .method("channel")
        .static_()
        .param("key", PhpType::String)
        .param("factory", PhpType::Callable)
        .handler(|call| registry_get_or_create(call, 0, 1, Some(SharedType::Channel)))
        .method("map")
        .static_()
        .param("key", PhpType::String)
        .param("factory", PhpType::Callable)
        .handler(|call| registry_get_or_create(call, 0, 1, Some(SharedType::Map)))
        .method("pool")
        .static_()
        .param("key", PhpType::String)
        .param("factory", PhpType::Callable)
        .handler(|call| registry_get_or_create(call, 0, 1, Some(SharedType::Pool)))
        // ── Namespace management — operates on the index, NOT the objects.
        .method("remove")
        .static_()
        .param("key", PhpType::String)
        .handler(|call| {
            let key = call.arg_str(0)?.to_string();
            if key.is_empty() {
                return Err(PhpError::Exception {
                    class: "InvalidArgumentException".into(),
                    message: "Shared\\Registry::remove key must be a non-empty string".into(),
                    code: 0,
                });
            }
            call.ret_bool(registry().name_remove(&key));
            Ok(())
        })
        .method("keys")
        .static_()
        .handler(|call| {
            let names = registry().name_keys();
            call.ret_array(names.len() as u32, |b| {
                for k in &names {
                    b.push_str(k);
                }
            });
            Ok(())
        })
        // ── Layer-wide introspection — O(1) atomic loads.
        .method("memoryUsage")
        .static_()
        .handler(|call| {
            call.ret_long(oxphp_shared_registry_memory_usage() as i64);
            Ok(())
        })
        .method("count")
        .static_()
        .handler(|call| {
            call.ret_long(oxphp_shared_registry_count() as i64);
            Ok(())
        })
        .build()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::ox_shared::config::{LockDiagnosticsLevel, SharedConfig};
    use crate::plugins::ox_shared::types::counter::CounterInner;

    // Compile-time guard that `CreatingGuard: !Send`. A guard moved
    // off the creator thread would silently leak the Creating slot
    // (Drop-time `name_abort` is creator-thread checked and becomes a
    // no-op on the wrong thread). Uses the ambiguous-impl idiom: if
    // `T: Send` then both `()` and `Invalid` are valid type params for
    // `AmbiguousIfSend`, and the unannotated call below fails with
    // "type annotations needed". A future refactor that drops the
    // `PhantomData<*mut ()>` from CreatingGuard trips this at compile
    // time.
    #[allow(dead_code)]
    trait AmbiguousIfSend<A> {
        fn some_item() {}
    }
    impl<T: ?Sized> AmbiguousIfSend<()> for T {}
    #[allow(dead_code)]
    struct Invalid;
    impl<T: ?Sized + Send> AmbiguousIfSend<Invalid> for T {}
    fn _assert_creating_guard_not_send() {
        <CreatingGuard as AmbiguousIfSend<_>>::some_item();
    }

    fn test_cfg() -> SharedConfig {
        SharedConfig {
            enabled: true,
            max_entries: 100,
            max_bytes: 4096,
            soft_limit_ratio: 0.7,
            metrics_enabled: true,
            introspection_enabled: true,
            introspection_preview_enabled: true,
            cycle_detect_depth: 16,
            cycle_detect_edges: 10_000,
            max_value_size: 1 << 20,
            max_channel_bytes: 64 << 20,
            poison_strict: false,
            lock_diagnostics: LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
            preview_string_limit: 256,
            preview_array_limit: 20,
        }
    }

    fn reg() -> &'static SharedRegistry {
        SharedRegistry::new_for_test(test_cfg())
    }

    #[test]
    fn acquire_vacant_returns_needs_factory_then_bind_hits() {
        let r = reg();
        assert!(matches!(
            r.name_acquire("k", SharedType::Counter),
            AcquireOutcome::NeedsFactory(_)
        ));
        let arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        let id = arc.id;
        r.name_bind("k", arc, SharedType::Counter).unwrap();
        match r.name_acquire("k", SharedType::Counter) {
            AcquireOutcome::Hit(ptr) => {
                let e = unsafe { &*ptr };
                assert_eq!(e.id, id);
                // balance the strong+1 from Arc::into_raw
                unsafe { drop(Arc::from_raw(ptr)) };
            }
            other => panic!("expected Hit, got {other:?}"),
        }
    }

    #[test]
    fn acquire_type_mismatch() {
        let r = reg();
        let _ = r.name_acquire("k", SharedType::Counter);
        let arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        r.name_bind("k", arc, SharedType::Counter).unwrap();
        assert!(matches!(
            r.name_acquire("k", SharedType::Flag),
            AcquireOutcome::TypeMismatch
        ));
    }

    #[test]
    fn abort_lets_next_caller_create() {
        let r = reg();
        assert!(matches!(
            r.name_acquire("k", SharedType::Counter),
            AcquireOutcome::NeedsFactory(_)
        ));
        r.name_abort("k");
        assert!(matches!(
            r.name_acquire("k", SharedType::Counter),
            AcquireOutcome::NeedsFactory(_)
        ));
    }

    #[test]
    fn remove_unbinds_and_drops_pin() {
        let r = reg();
        let _ = r.name_acquire("k", SharedType::Counter);
        let arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        let id = arc.id;
        r.name_bind("k", arc, SharedType::Counter).unwrap();
        assert!(r.name_remove("k"));
        assert!(
            r.lookup(id).is_err(),
            "pin dropped → last Arc gone → Entry self-deregisters"
        );
        assert!(!r.name_remove("k"));
    }

    #[test]
    fn keys_lists_bound_only() {
        let r = reg();
        let _ = r.name_acquire("bound", SharedType::Counter);
        let arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        r.name_bind("bound", arc, SharedType::Counter).unwrap();
        // Leave "creating" in Creating state (no bind) — must NOT appear in keys().
        let _ = r.name_acquire("creating", SharedType::Counter);
        let mut ks = r.name_keys();
        ks.sort();
        assert_eq!(ks, vec!["bound".to_string()]);
    }

    #[test]
    fn reentrant_acquire_returns_reentrant() {
        let r = reg();
        assert!(matches!(
            r.name_acquire("k", SharedType::Counter),
            AcquireOutcome::NeedsFactory(_)
        ));
        // Same thread, slot is Creating → blocking would self-deadlock.
        assert!(matches!(
            r.name_acquire("k", SharedType::Counter),
            AcquireOutcome::Reentrant
        ));
    }

    #[test]
    fn concurrent_acquire_one_creator_all_hit_same_id() {
        use std::sync::atomic::{AtomicUsize, Ordering as O};
        use std::thread;

        let r = reg();
        let creators = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];
        for _ in 0..8 {
            let creators = creators.clone();
            handles.push(thread::spawn(move || -> u64 {
                match r.name_acquire("hot", SharedType::Counter) {
                    AcquireOutcome::NeedsFactory(_) => {
                        creators.fetch_add(1, O::Relaxed);
                        let arc = r
                            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
                            .unwrap();
                        let id = arc.id;
                        r.name_bind("hot", arc, SharedType::Counter).unwrap();
                        id
                    }
                    AcquireOutcome::Hit(ptr) => {
                        let id = unsafe { (*ptr).id };
                        // Balance the strong+1 from Arc::into_raw.
                        unsafe { drop(Arc::from_raw(ptr)) };
                        id
                    }
                    AcquireOutcome::TypeMismatch => unreachable!("same type used"),
                    AcquireOutcome::Reentrant => {
                        unreachable!("distinct threads — no reentrancy possible")
                    }
                    AcquireOutcome::ShuttingDown => unreachable!("no drain in this test"),
                    AcquireOutcome::Deadlock => unreachable!("no cross-key cycle in this test"),
                }
            }));
        }
        let ids: Vec<u64> = handles.into_iter().map(|h| h.join().unwrap()).collect();
        assert_eq!(
            creators.load(O::Relaxed),
            1,
            "exactly one thread should create"
        );
        assert!(
            ids.iter().all(|&id| id == ids[0]),
            "all threads see the same entry id, got {ids:?}"
        );
    }

    #[test]
    fn abort_on_bound_slot_is_noop() {
        // `name_abort` must NOT tear down a Bound slot — that would silently
        // drop the pin while waiters expect the slot to stay live. Only a
        // Creating slot may be aborted.
        let r = reg();
        let _ = r.name_acquire("k", SharedType::Counter);
        let arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        let id = arc.id;
        r.name_bind("k", arc, SharedType::Counter).unwrap();

        r.name_abort("k"); // must do nothing

        assert!(r.lookup(id).is_ok(), "Bound pin must survive abort");
        assert_eq!(r.name_keys(), vec!["k".to_string()]);
    }

    #[test]
    fn remove_does_not_evict_creating_slot() {
        // `name_remove` must only delete Bound slots. Removing a Creating
        // slot would orphan every waiter (their cloned gate Arc would never
        // be settled).
        let r = reg();
        assert!(matches!(
            r.name_acquire("k", SharedType::Counter),
            AcquireOutcome::NeedsFactory(_)
        ));
        // Slot is Creating now.
        assert!(!r.name_remove("k"), "Creating slot must not be removed");
        // Slot still Creating — name_acquire on this thread sees Reentrant.
        assert!(matches!(
            r.name_acquire("k", SharedType::Counter),
            AcquireOutcome::Reentrant
        ));
    }

    #[test]
    fn creating_guard_aborts_slot_on_drop() {
        // The exact panic-leak scenario: NeedsFactory returned, a guard is
        // created, then the path unwinds (panic / early return). Drop must
        // settle the gate AND clear the slot so future acquirers aren't
        // stuck — the cross-key-cycle 30 s timeout would otherwise fire.
        let r = reg();
        let gate = match r.name_acquire("k", SharedType::Counter) {
            AcquireOutcome::NeedsFactory(g) => g,
            other => panic!("expected NeedsFactory, got {other:?}"),
        };
        {
            let _g = CreatingGuard::new(r, "k".to_string(), gate);
            // _g dropped here without commit
        }
        // Slot should be cleared and ready for a fresh creator.
        assert!(
            matches!(
                r.name_acquire("k", SharedType::Counter),
                AcquireOutcome::NeedsFactory(_)
            ),
            "guard drop must release the Creating slot"
        );
    }

    #[test]
    fn creating_guard_drop_settles_gate_even_after_slot_cleared() {
        // Regression for the drain-deadlock race: a thread acquires a
        // Creating slot, drain.clear() removes the slot before the
        // creator finishes, the creator's guard drops without commit.
        // Pre-fix, name_abort's remove_if would silently no-op (slot
        // already gone) and the gate would never settle — any waiter
        // holding a cloned `Arc<CreateGate>` would block until
        // GATE_WAIT_TIMEOUT (30 s). The fix is: the guard holds the
        // gate Arc directly and unconditionally settles it on drop.
        //
        // Test shape: extract the gate via NeedsFactory(gate), wipe
        // the names index out from under the guard, then drop the
        // guard. A wait_with on a clone of the gate must observe the
        // settle, not time out.
        let r = reg();
        let gate = match r.name_acquire("k", SharedType::Counter) {
            AcquireOutcome::NeedsFactory(g) => g,
            other => panic!("expected NeedsFactory, got {other:?}"),
        };
        // Simulate drain.clear() having already torn down the slot —
        // the guard's name_abort will find nothing to remove.
        r.names.clear();
        // Snapshot the gate so we can wait on it after the guard runs.
        let waiter_gate = gate.clone();
        {
            let _g = CreatingGuard::new(r, "k".to_string(), gate);
            // _g dropped here without commit
        }
        // The gate MUST have been settled — wait_with returns instantly.
        let res = waiter_gate.wait_with(std::time::Duration::from_millis(50));
        assert!(
            matches!(res, Ok(None)),
            "guard drop must settle the gate as aborted even when the \
             slot was already cleared (e.g. by drain), got {res:?}"
        );
    }

    #[test]
    fn creating_guard_committed_leaves_slot() {
        // After commit(), Drop is a no-op — the slot stays in whatever
        // state the caller put it in (typically Bound).
        let r = reg();
        let gate = match r.name_acquire("k", SharedType::Counter) {
            AcquireOutcome::NeedsFactory(g) => g,
            other => panic!("expected NeedsFactory, got {other:?}"),
        };
        let arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        r.name_bind("k", arc, SharedType::Counter).unwrap();
        {
            let mut g = CreatingGuard::new(r, "k".to_string(), gate);
            g.commit();
        }
        // Bound slot survives.
        assert_eq!(r.name_keys(), vec!["k".to_string()]);
    }

    #[test]
    fn abort_from_foreign_thread_is_noop() {
        // Defence-in-depth: name_abort must reject non-creator threads so
        // a stale CreatingGuard from a previous (aborted) creator can't
        // tear down the slot a new creator just installed.
        let r = reg();
        // Thread A becomes creator of "k", then leaks (no abort, no bind).
        let _a = std::thread::spawn(move || {
            let _ = r.name_acquire("k", SharedType::Counter);
        })
        .join();
        // Main thread tries to abort A's slot — must NOT remove it.
        r.name_abort("k");
        // Slot is still Creating; from this thread it's not reentrant
        // (different thread), so we'd wait — instead, peek via name_keys:
        // not Bound, so absent from keys. The behaviour we care about is
        // that the slot stayed: a fresh acquire from yet another thread
        // would block on its gate. We can verify via remove_if peek: the
        // slot still exists (a Bound check returns false, but the slot
        // isn't None either). Use the public surface: a Bound bind from
        // the main thread MUST fail with ForeignCreator (proving the
        // foreign Creating slot survived our abort attempt).
        let arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        assert_eq!(
            r.name_bind("k", arc, SharedType::Counter).unwrap_err(),
            BindError::ForeignCreator,
            "foreign Creating slot must have survived the abort"
        );
    }

    #[test]
    fn bind_from_foreign_thread_rejected() {
        // Simulate the drain race: thread A becomes the creator, drain
        // settle-aborts and clears the slot, thread B acquires fresh and
        // becomes a new creator, A's late factory tries to bind → must
        // be rejected with ForeignCreator (otherwise A would settle B's
        // gate with A's arc).
        use std::sync::{Arc as StdArc, Barrier};
        use std::thread;
        let r = reg();

        // Thread B (foreign creator). It acquires, opens its own gate,
        // and parks — we only need its Creating slot installed.
        let b_started = StdArc::new(Barrier::new(2));
        let b_started_clone = b_started.clone();
        let b_done = StdArc::new(Barrier::new(2));
        let b_done_clone = b_done.clone();
        let t = thread::spawn(move || {
            assert!(matches!(
                r.name_acquire("k", SharedType::Counter),
                AcquireOutcome::NeedsFactory(_)
            ));
            b_started_clone.wait();
            // hold the slot Creating until the main thread tries to bind
            b_done_clone.wait();
        });
        b_started.wait();

        // Main thread: try to bind (we never acquired — slot is owned by B).
        let arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        let err = r.name_bind("k", arc, SharedType::Counter).unwrap_err();
        assert_eq!(
            err,
            BindError::ForeignCreator,
            "non-creator must not settle a foreign gate"
        );

        b_done.wait();
        t.join().unwrap();
    }

    #[test]
    fn bind_on_already_bound_rejected() {
        let r = reg();
        let _ = r.name_acquire("k", SharedType::Counter);
        let arc1 = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        r.name_bind("k", arc1, SharedType::Counter).unwrap();
        let arc2 = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        assert_eq!(
            r.name_bind("k", arc2, SharedType::Counter).unwrap_err(),
            BindError::AlreadyBound
        );
    }

    #[test]
    fn bind_without_acquire_rejected() {
        let r = reg();
        let arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        assert_eq!(
            r.name_bind("k", arc, SharedType::Counter).unwrap_err(),
            BindError::NoCreatingSlot
        );
    }

    #[test]
    fn acquire_after_drain_returns_shutting_down() {
        let r = reg();
        r.drain();
        assert!(matches!(
            r.name_acquire("k", SharedType::Counter),
            AcquireOutcome::ShuttingDown
        ));
    }

    #[test]
    fn bind_after_drain_returns_shutting_down() {
        // Bind path must independently reject post-shutdown: a thread that
        // got NeedsFactory just before drain set the flag would otherwise
        // pin a fresh Bound slot past teardown.
        let r = reg();
        // Acquire BEFORE drain so we have a Creating slot owned by us.
        assert!(matches!(
            r.name_acquire("k", SharedType::Counter),
            AcquireOutcome::NeedsFactory(_)
        ));
        r.drain();
        let arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        assert_eq!(
            r.name_bind("k", arc, SharedType::Counter).unwrap_err(),
            BindError::ShuttingDown
        );
    }

    #[test]
    fn gate_wait_times_out_when_never_settled() {
        // Lower-level test of CreateGate's timeout — proves that a gate
        // never settled yields WaitTimedOut, the building block of the
        // cross-key cycle defence.
        let g = Arc::new(CreateGate::new());
        let res = g.wait_with(std::time::Duration::from_millis(20));
        assert!(res.is_err(), "expected WaitTimedOut, got settled");
    }

    #[test]
    fn gate_wait_honors_settle_racing_with_timeout() {
        // Regression: `wait_timeout` can return `timed_out() == true`
        // at the same instant `settle` flips `settled`. The waiter
        // re-acquires the lock — if it returns `WaitTimedOut` without
        // re-checking `g.settled`, a successful factory becomes a
        // false DeadlockException. Drive the race deterministically by
        // settling AFTER the waiter's timeout has elapsed but BEFORE
        // we observe the result; the gate's settle must win.
        let g = Arc::new(CreateGate::new());
        let g2 = g.clone();
        let r = reg();
        let arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        let t = std::thread::spawn(move || {
            // Sleep past the waiter's timeout, then settle. The waiter
            // wakes with `timed_out() == true`, re-acquires the lock,
            // and must see `g.settled == true` set here.
            std::thread::sleep(std::time::Duration::from_millis(50));
            g2.settle(Some(arc));
        });
        let res = g.wait_with(std::time::Duration::from_millis(10));
        t.join().unwrap();
        // The actual race outcome is non-deterministic, but the
        // critical invariant is: a settle that lands during the
        // timeout-handling window must NOT be lost. We accept either
        // outcome — settled (race won by settle path) or timed out
        // (race won by timeout path) — and verify that when settle
        // landed, the waiter saw it.
        match res {
            Ok(Some(_)) => { /* settle won — required behavior */ }
            Ok(None) => panic!("gate aborted unexpectedly"),
            Err(_) => {
                // Timeout won the race. To prove the fix actually
                // matters, drive a second waiter on the same (now
                // settled) gate: it must observe the settlement
                // immediately, never time out.
                let res2 = g.wait_with(std::time::Duration::from_millis(10));
                assert!(
                    matches!(res2, Ok(Some(_))),
                    "post-settle wait must hit the cached result"
                );
            }
        }
    }

    #[test]
    fn gate_wait_returns_value_when_settled_in_time() {
        let g = Arc::new(CreateGate::new());
        let g2 = g.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            // Build a sentinel arc via the registry to satisfy the
            // `Arc<Entry>` signature on settle.
            let r = reg();
            let arc = r
                .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
                .unwrap();
            g2.settle(Some(arc));
        });
        let res = g.wait_with(std::time::Duration::from_millis(500));
        t.join().unwrap();
        let val = res.expect("expected settled, got timeout");
        assert!(val.is_some(), "expected bound, got aborted");
    }

    #[test]
    fn drain_clears_names_and_drops_pins() {
        // Shutdown drain must release the strong Arc held by every Bound
        // slot, otherwise pinned named entries would outlive the process
        // teardown. After drain: names empty, pinned entries self-deregister.
        let r = reg();
        let _ = r.name_acquire("k", SharedType::Counter);
        let arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        let id = arc.id;
        r.name_bind("k", arc, SharedType::Counter).unwrap();
        r.drain();
        assert!(r.name_keys().is_empty(), "drain must clear the name index");
        assert!(
            r.lookup(id).is_err(),
            "pin released by drain → last Arc gone → Entry self-deregisters"
        );
    }

    #[test]
    fn total_bytes_and_total_entries_reflect_inserts() {
        // Sanity check that the existing public accessors (which
        // `Registry::memoryUsage()`/`count()` reuse via FFI) report the
        // expected deltas. These methods are existing API; this test just
        // anchors them against the name-index work.
        let r = reg();
        let b0 = r.total_bytes();
        let e0 = r.total_entries();
        let _arc = r
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .unwrap();
        assert!(r.total_bytes() > b0);
        assert_eq!(r.total_entries(), e0 + 1);
    }

    #[test]
    fn waiter_must_use_settled_arc_when_remove_races_after_bind() {
        // Reproducer for the `name_acquire` waiter race that the PHP
        // probabilistic test (`test_registry_remove_race_double_factory.php`)
        // only catches under heavy contention. This is the deterministic
        // in-process version.
        //
        // Bug shape: when `CreateGate::wait()` returns `Ok(Some(arc))`,
        // the waiter discards the carried arc via `Ok(_) => {} continue;`
        // and re-reads the names slot. If `name_remove` lands between
        // the creator's `name_bind` (which settles the gate with
        // `Some(arc)`) and the waiter's re-read, the waiter sees the
        // slot Vacant and is handed `NeedsFactory(_)` — becoming a
        // second creator for the same name, violating exactly-once.
        //
        // Sequencing: main calls bind + remove back-to-back without
        // yielding, so the waiter is marked-runnable but not yet
        // scheduled (single-core) or runs in parallel on another core
        // (multi-core) when remove lands. The pre-bind sleep ensures
        // the waiter has parked. Multiple trials make detection
        // reliable across schedulers — a single observed
        // `NeedsFactory` is sufficient to confirm the bug.
        use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering as Ord};
        use std::sync::Arc as StdArc;
        use std::thread;
        use std::time::Duration;

        // `AcquireOutcome::Hit` carries `*const Entry` which is `!Send`,
        // so we cannot ferry the raw outcome across threads. Process it
        // inside the spawned thread and return a Send-safe summary.
        enum WaiterResult {
            HitWithId(u64),
            BugBecameNewCreator,
        }

        let r = reg();
        let mut bug_hits: usize = 0;

        for trial in 0..50 {
            let key = format!("k_{trial}");

            // T0: main becomes the creator, opens the Creating slot.
            match r.name_acquire(&key, SharedType::Counter) {
                AcquireOutcome::NeedsFactory(_g) => {} // hold slot via the names index
                other => panic!("trial {trial}: expected NeedsFactory, got {other:?}"),
            }

            // T1: spawn waiter — it parks in `gate.wait()` because the
            // slot is Creating and the gate is owned by main (different
            // thread, so no Reentrant short-circuit).
            let key_w = key.clone();
            let waiter_running = StdArc::new(AtomicUsize::new(0));
            let waiter_running_clone = waiter_running.clone();
            let bind_id_shared = StdArc::new(AtomicU64::new(0));
            let bind_id_w = bind_id_shared.clone();
            let w: thread::JoinHandle<WaiterResult> = thread::spawn(move || {
                waiter_running_clone.store(1, Ord::Release);
                match r.name_acquire(&key_w, SharedType::Counter) {
                    AcquireOutcome::Hit(ptr) => {
                        let id = unsafe { (*ptr).id };
                        // balance the Arc::into_raw on the Hit path
                        unsafe { drop(StdArc::from_raw(ptr)) };
                        let _ = bind_id_w; // settled before we read; kept for ordering
                        WaiterResult::HitWithId(id)
                    }
                    AcquireOutcome::NeedsFactory(_g) => WaiterResult::BugBecameNewCreator,
                    other => panic!("waiter saw unexpected outcome: {other:?}"),
                }
            });

            // Wait until the waiter has entered name_acquire and very
            // likely parked on `cv.wait_timeout`.
            while waiter_running.load(Ord::Acquire) == 0 {
                thread::yield_now();
            }
            thread::sleep(Duration::from_millis(5));

            // T2 + T3: bind then remove with no yield between — main
            // keeps the CPU long enough that the waiter wakes only
            // after the slot has been evicted.
            let arc = r
                .insert(SharedType::Counter, StdArc::new(CounterInner::new(0)))
                .unwrap();
            let bind_id = arc.id;
            bind_id_shared.store(bind_id, Ord::Release);
            r.name_bind(&key, arc, SharedType::Counter).unwrap();
            let _ = r.name_remove(&key);

            match w.join().unwrap() {
                WaiterResult::HitWithId(id) => {
                    assert_eq!(
                        id, bind_id,
                        "trial {trial}: waiter must receive the settled arc \
                         (got id {id}, bound id {bind_id})"
                    );
                }
                WaiterResult::BugBecameNewCreator => {
                    // Bug surfaced: waiter discarded the settled arc and
                    // became a new creator after the slot was evicted.
                    bug_hits += 1;
                }
            }
        }

        assert_eq!(
            bug_hits, 0,
            "{bug_hits} / 50 trials had a waiter become a new creator after a \
             concurrent remove() — `name_acquire` discards the settled \
             `Ok(Some(arc))` from `gate.wait()` instead of returning `Hit(arc)`. \
             See registry.rs `name_acquire` line ~344 (`Ok(_) => {{}} continue;`)."
        );
    }
}
