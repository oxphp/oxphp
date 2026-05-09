//! Global registry of per-worker cancellation slots. Each worker
//! registers its EG(vm_interrupt) address here on boot so other
//! threads can interrupt it. Each request stores a Weak back-ref
//! to its CancellationState so cross-thread cancellation can
//! find the right worker.

use std::sync::atomic::{AtomicPtr, Ordering};
use std::sync::{Mutex, OnceLock, Weak};

use crate::bridge::cancel::{CancelReason, CancellationState};

pub struct WorkerSlot {
    pub cancel_state: Mutex<Option<Weak<CancellationState>>>,
    pub interrupt_flag_ptr: AtomicPtr<u8>,
    pub heartbeat: crate::php::heartbeat::WorkerHeartbeat,
}

impl WorkerSlot {
    pub fn new() -> Self {
        Self {
            cancel_state: Mutex::new(None),
            interrupt_flag_ptr: AtomicPtr::new(std::ptr::null_mut()),
            heartbeat: crate::php::heartbeat::WorkerHeartbeat::new(),
        }
    }
}

impl Default for WorkerSlot {
    fn default() -> Self {
        Self::new()
    }
}

pub static WORKERS: OnceLock<Vec<WorkerSlot>> = OnceLock::new();

/// Idempotent — only the first call populates the registry.
pub fn init_workers(count: usize) {
    let _ = WORKERS.get_or_init(|| (0..count).map(|_| WorkerSlot::new()).collect());
}

/// Cross-thread cancellation: set the reason on the request's state
/// (first writer wins) and, if a worker holds it, raise vm_interrupt
/// on that worker. Safe to call when no worker holds it: only the
/// reason is recorded; the worker will observe it on receive (B).
pub fn cancel_request(state: &std::sync::Arc<CancellationState>, reason: CancelReason) {
    if !state.set(reason) {
        return;
    }
    let workers = match WORKERS.get() {
        Some(w) => w,
        None => return,
    };
    for slot in workers.iter() {
        // Resolve the target's interrupt-flag pointer under the lock,
        // then DROP the guard before writing across threads. Holding
        // the Mutex during the cross-thread store would deadlock the
        // disconnect path if the worker panics and poisons it; the
        // Mutex's job is only to keep the Weak<>→Arc<> upgrade
        // race-free, not to serialise the interrupt write itself.
        let interrupt_addr = {
            let guard = slot.cancel_state.lock().unwrap();
            match guard.as_ref().and_then(Weak::upgrade) {
                Some(active) if std::sync::Arc::ptr_eq(&active, state) => {
                    slot.interrupt_flag_ptr.load(Ordering::Acquire)
                }
                _ => continue,
            }
            // guard dropped here at end of scope
        };
        if !interrupt_addr.is_null() {
            // SAFETY: cross-thread Zend interrupt pattern. Routed
            // through the bridge so the underlying `zend_atomic_bool`
            // is mutated via `zend_atomic_bool_store_ex`, not aliased
            // as a plain `uint8_t*`. The TLS byte lives for the
            // worker thread's lifetime.
            unsafe {
                crate::bridge::ffi::oxphp_bridge_request_interrupt_at(
                    interrupt_addr as *mut std::os::raw::c_void,
                );
            }
        }
        return;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn init_is_idempotent() {
        init_workers(4);
        init_workers(8); // ignored — first call wins
        assert_eq!(WORKERS.get().unwrap().len(), 4);
    }

    #[test]
    fn cancel_with_no_active_worker_records_reason() {
        let state = Arc::new(CancellationState::new());
        cancel_request(&state, CancelReason::ClientAbort);
        assert_eq!(state.get(), CancelReason::ClientAbort);
    }

    #[test]
    fn second_cancel_is_noop() {
        let state = Arc::new(CancellationState::new());
        cancel_request(&state, CancelReason::ClientAbort);
        cancel_request(&state, CancelReason::Timeout);
        assert_eq!(state.get(), CancelReason::ClientAbort);
    }
}
