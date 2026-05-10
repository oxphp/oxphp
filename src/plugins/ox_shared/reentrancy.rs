//! Thread-local held-mutex set for intra-thread deadlock detection
//! and lock-order-hazard warnings.

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use smallvec::SmallVec;

use crate::plugins::ox_shared::error::{set_last_error, SharedError};
use crate::plugins::ox_shared::registry::SharedId;

thread_local! {
    // Currently-held mutex ids on this thread. Typical nesting depth
    // is 0–2; the inline capacity of 8 avoids allocations in all but
    // pathological cases.
    static HELD_MUTEXES: RefCell<SmallVec<[SharedId; 8]>> = const {
        RefCell::new(SmallVec::new_const())
    };

    // Rate limiter for lock-order-hazard log emission. One event per
    // thread per 60 seconds of wall time.
    static LAST_LOCK_ORDER_WARN: Cell<Option<Instant>> = const { Cell::new(None) };
}

const LOCK_ORDER_WARN_INTERVAL: Duration = Duration::from_secs(60);

/// Attempt to register `id` as held on this thread. Returns `Err(Deadlock)`
/// if the thread already holds this id (recursive acquire).
///
/// On successful push, if total holds >= 2, emit a rate-limited
/// lock-order-hazard warn via tracing.
pub fn push_held(id: SharedId) -> Result<(), SharedError> {
    HELD_MUTEXES.with(|h| {
        let mut v = h.borrow_mut();
        if v.contains(&id) {
            set_last_error(format!(
                "recursive Mutex acquire on id={id} from a thread already holding it"
            ));
            return Err(SharedError::Deadlock);
        }
        v.push(id);
        if v.len() >= 2 {
            emit_rate_limited_warn(&v);
        }
        Ok(())
    })
}

/// Pop the most recent occurrence of `id` from the held set. Safe to
/// call from Drop; idempotent when id is not in the set.
pub fn pop_held(id: SharedId) {
    HELD_MUTEXES.with(|h| {
        let mut v = h.borrow_mut();
        if let Some(pos) = v.iter().rposition(|x| *x == id) {
            v.swap_remove(pos);
        }
    });
}

/// Count of currently-held mutexes on this thread. Used by tests
/// and /__ox_shared/preview diagnostics.
pub fn held_count() -> usize {
    HELD_MUTEXES.with(|h| h.borrow().len())
}

/// RAII guard that pops on drop (panic-safe).
pub struct MutexPopGuard(pub SharedId);
impl Drop for MutexPopGuard {
    fn drop(&mut self) {
        pop_held(self.0);
    }
}

fn emit_rate_limited_warn(stack: &[SharedId]) {
    let now = Instant::now();
    let should_emit = LAST_LOCK_ORDER_WARN.with(|last| {
        let previous = last.get();
        let fresh = previous.is_none_or(|t| now.duration_since(t) >= LOCK_ORDER_WARN_INTERVAL);
        if fresh {
            last.set(Some(now));
        }
        fresh
    });
    if should_emit {
        tracing::warn!(
            event = "shared.lock_order_hazard",
            nesting_depth = stack.len(),
            holding = ?stack,
            "deep Mutex nesting — consider reordering or merging critical sections"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_and_pop() {
        // Isolated thread to avoid TLS cross-test contamination.
        let h = std::thread::spawn(|| {
            assert_eq!(held_count(), 0);
            push_held(10).unwrap();
            assert_eq!(held_count(), 1);
            push_held(20).unwrap();
            assert_eq!(held_count(), 2);
            pop_held(20);
            assert_eq!(held_count(), 1);
            pop_held(10);
            assert_eq!(held_count(), 0);
        });
        h.join().unwrap();
    }

    #[test]
    fn recursive_push_errors() {
        let h = std::thread::spawn(|| {
            push_held(42).unwrap();
            let err = push_held(42).unwrap_err();
            assert_eq!(err, SharedError::Deadlock);
            pop_held(42);
        });
        h.join().unwrap();
    }

    #[test]
    fn pop_guard_cleans_up_on_drop() {
        let h = std::thread::spawn(|| {
            {
                let _g = {
                    push_held(5).unwrap();
                    MutexPopGuard(5)
                };
                assert_eq!(held_count(), 1);
            }
            assert_eq!(held_count(), 0);
        });
        h.join().unwrap();
    }
}
