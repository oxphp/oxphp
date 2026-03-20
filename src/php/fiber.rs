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

// ── C FFI callbacks ──────────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

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
}
