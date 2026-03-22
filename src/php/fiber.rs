//! Timer service for fiber-based cooperative scheduling.
//! Uses Instant-based deadlines (no Tokio dependency for basic timer tracking).

use std::cell::RefCell;
use std::time::{Duration, Instant};

/// Per-thread timer state for the fiber scheduler.
struct TimerState {
    next_id: u64,
    timers: Vec<(u64, Instant)>,
}

impl TimerState {
    fn new() -> Self {
        Self {
            next_id: 1,
            timers: Vec::new(),
        }
    }
}

thread_local! {
    static TIMER_STATE: RefCell<TimerState> = RefCell::new(TimerState::new());
}

/// Reset timer state — useful for testing and per-request cleanup.
pub fn init_timer_state() {
    TIMER_STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.next_id = 1;
        s.timers.clear();
    });
}

/// Register a timer that fires after `duration_ms` milliseconds.
/// Returns a unique timer ID (always > 0).
pub fn register_timer(duration_ms: u64) -> u64 {
    TIMER_STATE.with(|state| {
        let mut s = state.borrow_mut();
        let id = s.next_id;
        s.next_id += 1;
        let deadline = Instant::now() + Duration::from_millis(duration_ms);
        s.timers.push((id, deadline));
        id
    })
}

/// Check if a specific timer has fired (deadline has passed).
pub fn is_timer_ready(id: u64) -> bool {
    TIMER_STATE.with(|state| {
        let s = state.borrow();
        let now = Instant::now();
        s.timers
            .iter()
            .any(|(tid, deadline)| *tid == id && now >= *deadline)
    })
}

/// Return all fired timer IDs and remove them from the list.
pub fn poll_ready_timers() -> Vec<u64> {
    TIMER_STATE.with(|state| {
        let mut s = state.borrow_mut();
        let now = Instant::now();
        let mut ready = Vec::new();
        s.timers.retain(|(id, deadline)| {
            if now >= *deadline {
                ready.push(*id);
                false
            } else {
                true
            }
        });
        ready
    })
}

/// Remove a timer without firing it.
pub fn remove_timer(id: u64) {
    TIMER_STATE.with(|state| {
        let mut s = state.borrow_mut();
        s.timers.retain(|(tid, _)| *tid != id);
    });
}

// ── C FFI callbacks (timers) ──────────────────────────────────

/// FFI callback: register a timer from C, returns timer ID.
///
/// # Safety
/// Called from C bridge code on the same thread.
#[no_mangle]
pub unsafe extern "C" fn timer_register_callback(duration_ms: u64) -> u64 {
    register_timer(duration_ms)
}

/// FFI callback: write ready timer IDs into a C buffer.
/// Returns the number of IDs written (up to `max_count`).
///
/// # Safety
/// `out_ids` must point to a buffer of at least `max_count` u64 elements.
#[no_mangle]
pub unsafe extern "C" fn timer_poll_callback(out_ids: *mut u64, max_count: u32) -> u32 {
    let ready = poll_ready_timers();
    let count = ready.len().min(max_count as usize);
    for (i, id) in ready.iter().take(count).enumerate() {
        unsafe { *out_ids.add(i) = *id };
    }
    count as u32
}

/// FFI callback: remove a timer by ID.
///
/// # Safety
/// Called from C bridge code on the same thread.
#[no_mangle]
pub unsafe extern "C" fn timer_remove_callback(id: u64) {
    remove_timer(id);
}

// ═══════════════════════════════════════════════════════════════
//  Per-Fiber TLS Slot Management
//
//  When the fiber scheduler switches between fibers, it must save the
//  current fiber's Rust-side TLS and restore the target fiber's TLS.
//  This module manages a per-thread HashMap of fiber_id -> FiberTlsSlot.
// ═══════════════════════════════════════════════════════════════

use std::collections::HashMap;

#[cfg(feature = "php")]
use super::sapi::{ResponseBuffers, RESPONSE};

/// Standalone definition of ResponseBuffers for non-php builds (host testing).
/// When `feature = "php"` is enabled, this is imported from `sapi` instead.
#[cfg(not(feature = "php"))]
#[derive(Default)]
#[allow(dead_code)] // Fields used in tests
pub(crate) struct ResponseBuffers {
    pub(crate) output: Vec<u8>,
    pub(crate) headers: Vec<(String, String)>,
    pub(crate) status_code: u16,
}

/// Saved per-fiber TLS state.
///
/// Saves RESPONSE, EARLY_TX, and WORKER_REQUEST_START per fiber.
/// REQUEST_DATA is not yet saved (TODO — needs CString pointer safety analysis).
#[allow(dead_code)]
struct FiberTlsSlot {
    response: ResponseBuffers,
    #[cfg(feature = "php")]
    early_tx: Option<(
        Instant,
        tokio::sync::oneshot::Sender<crate::types::ScriptResponse>,
    )>,
    #[cfg(feature = "php")]
    request_start: Option<Instant>,
}

thread_local! {
    static FIBER_TLS_SLOTS: RefCell<HashMap<u64, FiberTlsSlot>> = RefCell::new(HashMap::new());
}

/// Save the current fiber's RESPONSE TLS into the slot map.
///
/// Uses `std::mem::take` to move the RESPONSE out of TLS (leaving a Default
/// placeholder) without cloning. The saved data is keyed by `fiber_id`.
pub fn save_fiber_tls(fiber_id: u64) {
    #[cfg(feature = "php")]
    {
        let response = RESPONSE.with(|r| std::mem::take(&mut *r.borrow_mut()));
        let early_tx = super::sapi::take_early_tx();
        let request_start = super::sapi::take_request_start();
        FIBER_TLS_SLOTS.with(|slots| {
            slots.borrow_mut().insert(
                fiber_id,
                FiberTlsSlot {
                    response,
                    early_tx,
                    request_start,
                },
            );
        });
    }
    #[cfg(not(feature = "php"))]
    {
        let _ = fiber_id;
    }
}

/// Restore a fiber's saved RESPONSE TLS from the slot map.
///
/// Swaps the slot's saved state back into the thread-local. If no slot
/// exists for this fiber_id (first activation), TLS is left unchanged.
pub fn restore_fiber_tls(fiber_id: u64) {
    #[cfg(feature = "php")]
    {
        FIBER_TLS_SLOTS.with(|slots| {
            if let Some(slot) = slots.borrow_mut().remove(&fiber_id) {
                RESPONSE.with(|r| {
                    *r.borrow_mut() = slot.response;
                });
                super::sapi::restore_early_tx(slot.early_tx);
                super::sapi::restore_request_start(slot.request_start);
            }
        });
    }
    #[cfg(not(feature = "php"))]
    {
        let _ = fiber_id;
    }
}

/// Remove a fiber's TLS slot (fiber completed or was destroyed).
pub fn remove_fiber_tls(fiber_id: u64) {
    FIBER_TLS_SLOTS.with(|slots| {
        slots.borrow_mut().remove(&fiber_id);
    });
}

// ── C FFI callbacks (fiber TLS) ──────────────────────────────

/// FFI callback: save current fiber's TLS context.
///
/// # Safety
/// Called from C bridge code on the same thread.
#[no_mangle]
pub unsafe extern "C" fn fiber_save_ctx_callback(fiber_id: u64) {
    save_fiber_tls(fiber_id);
}

/// FFI callback: restore a fiber's TLS context.
///
/// # Safety
/// Called from C bridge code on the same thread.
#[no_mangle]
pub unsafe extern "C" fn fiber_restore_ctx_callback(fiber_id: u64) {
    restore_fiber_tls(fiber_id);
}

/// FFI callback: drop a fiber's TLS slot.
///
/// # Safety
/// Called from C bridge code on the same thread.
#[no_mangle]
pub unsafe extern "C" fn fiber_drop_ctx_callback(fiber_id: u64) {
    remove_fiber_tls(fiber_id);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    // ── Timer tests ──────────────────────────────────────────

    #[test]
    fn register_timer_returns_id() {
        init_timer_state();
        let id = register_timer(1000);
        assert!(id > 0, "timer ID must be > 0");
    }

    #[test]
    fn timer_not_ready_immediately() {
        init_timer_state();
        let id = register_timer(1000); // 1 second
        assert!(
            !is_timer_ready(id),
            "1s timer should not be ready immediately"
        );
    }

    #[test]
    fn expired_timer_is_ready() {
        init_timer_state();
        let id = register_timer(1); // 1ms
        thread::sleep(Duration::from_millis(10));
        assert!(
            is_timer_ready(id),
            "1ms timer should be ready after 10ms sleep"
        );
    }

    #[test]
    fn poll_returns_ready_timer_ids() {
        init_timer_state();
        let short = register_timer(1); // 1ms — will fire
        let long = register_timer(10_000); // 10s — will not fire
        thread::sleep(Duration::from_millis(10));

        let ready = poll_ready_timers();
        assert!(
            ready.contains(&short),
            "short timer should be in ready list"
        );
        assert!(
            !ready.contains(&long),
            "long timer should NOT be in ready list"
        );

        // After poll, the fired timer is removed
        assert!(!is_timer_ready(short), "polled timer should be removed");
    }

    #[test]
    fn remove_timer_cleans_up() {
        init_timer_state();
        let id = register_timer(1); // 1ms
        remove_timer(id);
        thread::sleep(Duration::from_millis(10));

        // Timer was removed, so it should not appear as ready
        assert!(!is_timer_ready(id), "removed timer should not be found");
        let ready = poll_ready_timers();
        assert!(
            !ready.contains(&id),
            "removed timer should not appear in poll"
        );
    }

    // ── Fiber TLS slot tests ─────────────────────────────────

    #[test]
    fn save_and_restore_response_slot() {
        // Fiber TLS slots work at the Rust level without PHP.
        // We test the slot map directly by saving/restoring ResponseBuffers.

        // Clear any leftover state from other tests.
        FIBER_TLS_SLOTS.with(|slots| slots.borrow_mut().clear());

        // Simulate fiber 1: save a slot with some data.
        let slot1 = FiberTlsSlot {
            response: ResponseBuffers {
                output: b"fiber1-output".to_vec(),
                headers: vec![("X-Fiber".to_string(), "1".to_string())],
                status_code: 200,
            },
            #[cfg(feature = "php")]
            early_tx: None,
            #[cfg(feature = "php")]
            request_start: None,
        };
        FIBER_TLS_SLOTS.with(|slots| {
            slots.borrow_mut().insert(1, slot1);
        });

        // Simulate fiber 2: save a slot with different data.
        let slot2 = FiberTlsSlot {
            response: ResponseBuffers {
                output: b"fiber2-output".to_vec(),
                headers: vec![("X-Fiber".to_string(), "2".to_string())],
                status_code: 404,
            },
            #[cfg(feature = "php")]
            early_tx: None,
            #[cfg(feature = "php")]
            request_start: None,
        };
        FIBER_TLS_SLOTS.with(|slots| {
            slots.borrow_mut().insert(2, slot2);
        });

        // Restore fiber 1 — verify its data is present in the removed slot.
        FIBER_TLS_SLOTS.with(|slots| {
            let slot = slots
                .borrow_mut()
                .remove(&1)
                .expect("fiber 1 slot should exist");
            assert_eq!(slot.response.output, b"fiber1-output");
            assert_eq!(slot.response.status_code, 200);
            assert_eq!(slot.response.headers.len(), 1);
            assert_eq!(slot.response.headers[0].1, "1");
        });

        // Restore fiber 2 — verify its data.
        FIBER_TLS_SLOTS.with(|slots| {
            let slot = slots
                .borrow_mut()
                .remove(&2)
                .expect("fiber 2 slot should exist");
            assert_eq!(slot.response.output, b"fiber2-output");
            assert_eq!(slot.response.status_code, 404);
            assert_eq!(slot.response.headers[0].1, "2");
        });

        // Both slots should now be empty.
        FIBER_TLS_SLOTS.with(|slots| {
            assert!(slots.borrow().is_empty(), "all slots should be removed");
        });
    }

    #[test]
    fn remove_fiber_tls_drops_slot() {
        FIBER_TLS_SLOTS.with(|slots| slots.borrow_mut().clear());

        let slot = FiberTlsSlot {
            response: ResponseBuffers {
                output: b"temp".to_vec(),
                headers: Vec::new(),
                status_code: 200,
            },
            #[cfg(feature = "php")]
            early_tx: None,
            #[cfg(feature = "php")]
            request_start: None,
        };
        FIBER_TLS_SLOTS.with(|slots| {
            slots.borrow_mut().insert(42, slot);
        });

        remove_fiber_tls(42);

        FIBER_TLS_SLOTS.with(|slots| {
            assert!(
                !slots.borrow().contains_key(&42),
                "slot 42 should be removed"
            );
        });
    }

    #[test]
    fn remove_nonexistent_fiber_is_noop() {
        FIBER_TLS_SLOTS.with(|slots| slots.borrow_mut().clear());
        // Should not panic.
        remove_fiber_tls(999);
    }

    #[test]
    fn restore_nonexistent_fiber_is_noop() {
        FIBER_TLS_SLOTS.with(|slots| slots.borrow_mut().clear());
        // Should not panic — TLS left unchanged.
        restore_fiber_tls(999);
    }

    #[test]
    fn response_buffers_default_is_empty() {
        let buf = ResponseBuffers::default();
        assert!(buf.output.is_empty());
        assert!(buf.headers.is_empty());
        assert_eq!(buf.status_code, 0);
    }
}
