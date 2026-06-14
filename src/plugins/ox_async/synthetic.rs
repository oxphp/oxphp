//! Synthetic promises: allocate a promise id without dispatching a task
//! to the async pool. Used by Shared primitives (Channel::recv(timeout),
//! Mutex::with(timeout)) to suspend a fiber on an arbitrary Rust waker
//! while reusing the existing `oxphp_bridge_fiber_await` plumbing.
//!
//! Design summary (integration chosen to keep sapi.rs changes minimal):
//!
//! - Public API speaks `PromisePayload` (`Value(Vec<u8>)` /
//!   `Exception(String, String)` / `Cancelled`), matching the spec.
//! - Internally, `alloc_and_register()` creates a
//!   `tokio::sync::oneshot::channel::<AsyncResult>()` — the SAME
//!   receiver type the existing `PROMISE_MAP` stores — and parks the
//!   receiver there via `sapi::register_synthetic_receiver`. The
//!   `PromisePayload`-shaped sender is held in this module's global
//!   `SENDERS` map.
//! - `resolve`/`reject`/`cancel` translate `PromisePayload` →
//!   `AsyncResult` and fire the receiver. The existing
//!   `await_dispatch_callback` then drains the same way as for a
//!   regular async-pool task.
//!
//! This design is strictly additive: it does not alter the
//! `PROMISE_MAP` state enum or any existing hot-path code.

use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::OnceLock;

use dashmap::DashMap;
use tokio::sync::oneshot;

use crate::async_types::AsyncResult;

/// Type-erased keepalive carried alongside a delivered value: pins nested
/// `Shared\*` entries alive until the receiver deserializes the bytes.
/// `None` in the common case. Concrete type is owned by the producing layer
/// (e.g. a Channel's `SmallVec<[SharedRefOwned; 1]>`).
pub type Keepalive = Option<Box<dyn std::any::Any + Send>>;

/// Payload delivered to a suspended fiber.
pub enum PromisePayload {
    /// Portable-serialised zval bytes (system-malloc'd buffer is built
    /// by this module, not by the caller). The receiver deserialises
    /// on its own thread. The optional second element is a type-erased
    /// keepalive (e.g. a Channel's `SharedRefOwned` list) that pins nested
    /// `Shared\*` entries alive until the receiver deserialises the bytes;
    /// it rides into `AsyncResult.keepalive`. `None` in the common case.
    Value(Vec<u8>, Keepalive),
    /// Exception to throw: (fqn, message).
    Exception(String, String),
    /// Cancelled — no value; suspend site decides how to surface this
    /// (typically as a Closed/Stale error).
    Cancelled,
}

impl std::fmt::Debug for PromisePayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Value(b, k) => f
                .debug_tuple("Value")
                .field(&b.len())
                .field(&k.is_some())
                .finish(),
            Self::Exception(c, m) => f.debug_tuple("Exception").field(c).field(m).finish(),
            Self::Cancelled => write!(f, "Cancelled"),
        }
    }
}

/// Process-global sender registry. Key = synthetic promise id
/// (negative — `i64::MIN + 1` upwards — so it cannot collide with
/// async-pool ids, which monotonically increase from 0).
fn senders() -> &'static DashMap<i64, oneshot::Sender<AsyncResult>> {
    static SENDERS: OnceLock<DashMap<i64, oneshot::Sender<AsyncResult>>> = OnceLock::new();
    SENDERS.get_or_init(DashMap::new)
}

/// Monotonic counter. Starts at `i64::MIN + 1` so synthetic ids stay
/// negative and distinguishable from async-pool ids (always positive).
/// After 2^63 − 2 allocations the counter wraps; at realistic rates
/// (millions/sec) that's tens of thousands of years, so not guarded
/// in release. A debug-build assert catches the theoretical case.
static NEXT_ID: AtomicI64 = AtomicI64::new(i64::MIN + 1);

/// Allocate a synthetic promise. Returns `(id, receiver)`.
///
/// Low-level primitive for tests and custom plumbing. Production
/// callers on a PHP worker thread should use [`alloc_and_register`],
/// which parks the receiver into the thread's `PROMISE_MAP` so
/// `oxphp_bridge_fiber_await(id, ...)` can drain it.
#[must_use]
#[allow(dead_code)]
pub fn alloc() -> (i64, oneshot::Receiver<AsyncResult>) {
    let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
    debug_assert!(id < 0, "synthetic promise ID overflowed");
    let (tx, rx) = oneshot::channel();
    senders().insert(id, tx);
    (id, rx)
}

/// Allocate a synthetic promise AND register the receiver with the
/// current thread's `PROMISE_MAP` so `oxphp_bridge_fiber_await(id, ...)`
/// picks it up. Returns the id to pass into fiber_await.
///
/// Must be called from a PHP worker thread (the one that will block
/// on fiber_await). Not callable from a tokio task.
#[cfg(feature = "php")]
pub fn alloc_and_register() -> i64 {
    let (id, rx) = alloc();
    // We cast i64 → u64 (reinterpret bits). Synthetic ids have the top
    // bit set, so they occupy a disjoint region of the u64 key space
    // from async-pool ids (which grow monotonically from 0).
    let cancelled = std::sync::Arc::new(crate::async_types::CancelShared::new());
    crate::php::sapi::register_synthetic_receiver(id as u64, rx, cancelled);
    id
}

/// Deliver a value. Returns `true` if delivered, `false` if the
/// promise was already consumed (resolve/reject/cancel already called),
/// or the receiver was dropped.
pub fn resolve(id: i64, payload: PromisePayload) -> bool {
    let Some((_, tx)) = senders().remove(&id) else {
        return false;
    };
    let result = payload_to_result(payload);
    tx.send(result).is_ok()
}

/// Deliver a value to a parked waiter, handing the payload **back** if
/// the promise was already resolved by someone else (e.g. its own
/// `recvTimeout` cancel won the race against this delivery).
///
/// Unlike [`resolve`], this never silently drops the payload: the sole
/// sender is removed first, and the `AsyncResult` is built and sent only
/// once we hold a sender whose receiver is still alive. A `Some(payload)`
/// return means "this waiter could not take it — try a sibling or
/// re-deposit, no message lost." A `None` return means the value was
/// delivered to a live receiver.
///
/// Two paths return the payload (`Some`) instead of consuming it:
/// - the promise was already resolved by another resolver (its own
///   `recvTimeout` cancel / close won the race) — sender already gone;
/// - the receiver was dropped while the sender lingered (the parked fiber
///   was torn down, e.g. request shutdown, without a cancel). There is no
///   consumer for THIS id, so the caller falls through to a live sibling
///   waiter or re-parks, keeping delivery loss-free.
///
/// This guards the dead-waiter race that [`resolve`] documents: a buffered
/// Channel item handed to a waiter that cancelled (or was torn down)
/// between the pop and the send must not vanish; the caller recovers it.
///
/// A nanosecond TOCTOU remains: if the receiver is dropped between the
/// `is_closed` check and `send`, the payload is consumed.
pub fn resolve_value(
    id: i64,
    payload: Vec<u8>,
    keepalive: Keepalive,
) -> Option<(Vec<u8>, Keepalive)> {
    let Some((_, tx)) = senders().remove(&id) else {
        // Already resolved (timeout / cancel / close won the race) —
        // hand the payload (and its keepalive) back to the caller to
        // re-deposit.
        return Some((payload, keepalive));
    };
    if tx.is_closed() {
        // Receiver already dropped (the parked fiber is gone). No consumer
        // for THIS id — hand the payload + keepalive back so the caller
        // tries a live sibling waiter or re-deposits it. Loss-free.
        return Some((payload, keepalive));
    }
    // We hold a sender with a live receiver. Build the result and deliver;
    // the keepalive rides along in `AsyncResult.keepalive`.
    let _ = tx.send(payload_to_result(PromisePayload::Value(payload, keepalive)));
    None
}

/// Reject with an exception class + message.
pub fn reject(id: i64, class_fqn: impl Into<String>, message: impl Into<String>) -> bool {
    resolve(
        id,
        PromisePayload::Exception(class_fqn.into(), message.into()),
    )
}

/// Cancel (e.g. timeout in the suspend-site). Fires the receiver with
/// `Cancelled` so the caller cleans up promptly.
pub fn cancel(id: i64) -> bool {
    resolve(id, PromisePayload::Cancelled)
}

/// Convert public `PromisePayload` into the internal `AsyncResult`
/// shape the existing await plumbing expects.
///
/// - `Value(bytes)` → `{ success: true, serialized_value: malloc-copy of bytes }`
/// - `Exception(fqn, msg)` → `{ success: false, exception_class: fqn, exception_message: msg }`
/// - `Cancelled` → `{ success: false, exception_class: "OxPHP\\Async\\AsyncException", exception_message: "cancelled" }`
fn payload_to_result(payload: PromisePayload) -> AsyncResult {
    match payload {
        PromisePayload::Value(bytes, keepalive) => {
            if bytes.is_empty() {
                AsyncResult {
                    success: true,
                    serialized_value: std::ptr::null_mut(),
                    serialized_value_len: 0,
                    exception_class: None,
                    exception_message: None,
                    keepalive,
                }
            } else {
                // The downstream `oxphp_portable_free(serialized_value)`
                // uses `libc::free`, so we must allocate via libc malloc.
                let len = bytes.len();
                let buf = unsafe { libc::malloc(len) as *mut u8 };
                if buf.is_null() {
                    return AsyncResult {
                        success: false,
                        serialized_value: std::ptr::null_mut(),
                        serialized_value_len: 0,
                        exception_class: Some("OxPHP\\Async\\AsyncException".to_string()),
                        exception_message: Some(
                            "synthetic promise: failed to allocate result buffer".to_string(),
                        ),
                        // Delivery failed; releasing the keepalive here is correct.
                        keepalive: None,
                    };
                }
                unsafe {
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf, len);
                }
                AsyncResult {
                    success: true,
                    serialized_value: buf,
                    serialized_value_len: len,
                    exception_class: None,
                    exception_message: None,
                    keepalive,
                }
            }
        }
        PromisePayload::Exception(class, message) => AsyncResult {
            success: false,
            serialized_value: std::ptr::null_mut(),
            serialized_value_len: 0,
            exception_class: Some(class),
            exception_message: Some(message),
            keepalive: None,
        },
        PromisePayload::Cancelled => AsyncResult {
            success: false,
            serialized_value: std::ptr::null_mut(),
            serialized_value_len: 0,
            exception_class: Some("OxPHP\\Async\\AsyncException".to_string()),
            exception_message: Some("synthetic promise cancelled".to_string()),
            keepalive: None,
        },
    }
}

// ─── C-ABI shims ────────────────────────────────────────────────────

#[cfg(feature = "php")]
extern "C" fn c_alloc() -> i64 {
    alloc_and_register()
}

#[cfg(feature = "php")]
extern "C" fn c_resolve(id: i64, payload_bytes: *const u8, payload_len: usize) -> i32 {
    let payload = if payload_bytes.is_null() || payload_len == 0 {
        PromisePayload::Value(Vec::new(), None)
    } else {
        let slice = unsafe { std::slice::from_raw_parts(payload_bytes, payload_len) };
        PromisePayload::Value(slice.to_vec(), None)
    };
    i32::from(resolve(id, payload))
}

#[cfg(feature = "php")]
extern "C" fn c_reject(
    id: i64,
    cls_fqn: *const std::os::raw::c_char,
    message: *const std::os::raw::c_char,
) -> i32 {
    let fqn = if cls_fqn.is_null() {
        String::from("Exception")
    } else {
        unsafe { std::ffi::CStr::from_ptr(cls_fqn) }
            .to_string_lossy()
            .into_owned()
    };
    let msg = if message.is_null() {
        String::new()
    } else {
        unsafe { std::ffi::CStr::from_ptr(message) }
            .to_string_lossy()
            .into_owned()
    };
    i32::from(reject(id, fqn, msg))
}

#[cfg(feature = "php")]
extern "C" fn c_cancel(id: i64) -> i32 {
    i32::from(cancel(id))
}

/// Called once at plugin init (on the main thread, before workers
/// spawn). Publishes the four Rust shims to the C bridge so that
/// future C-side integrations (PHP userland glue, Shared primitive
/// internals) can reach into `synthetic::*` through stable symbols.
#[cfg(feature = "php")]
pub fn register_with_bridge() {
    unsafe {
        crate::bridge::ffi::oxphp_bridge_set_async_synth_alloc(c_alloc);
        crate::bridge::ffi::oxphp_bridge_set_async_synth_resolve(c_resolve);
        crate::bridge::ffi::oxphp_bridge_set_async_synth_reject(c_reject);
        crate::bridge::ffi::oxphp_bridge_set_async_synth_cancel(c_cancel);
    }
}

/// No-op stub for non-PHP builds — the bridge FFI is unavailable and
/// there's no worker thread to register receivers on.
#[cfg(not(feature = "php"))]
pub fn register_with_bridge() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alloc_returns_monotonic_ids() {
        let (a, _) = alloc();
        let (b, _) = alloc();
        assert!(b > a, "ids monotonically increase ({} !> {})", b, a);
    }

    #[test]
    fn alloc_ids_are_negative() {
        let (id, _) = alloc();
        assert!(
            id < 0,
            "synthetic ids must be negative to avoid async-pool collision"
        );
    }

    #[tokio::test]
    async fn resolve_delivers_value() {
        let (id, rx) = alloc();
        assert!(resolve(id, PromisePayload::Value(vec![1, 2, 3], None)));
        let result = rx.await.expect("receiver should receive");
        assert!(result.success);
        assert_eq!(result.serialized_value_len, 3);
        // Clean up the malloc'd buffer (AsyncResult::drop handles this).
    }

    #[tokio::test]
    async fn resolve_empty_value_uses_null_buf() {
        let (id, rx) = alloc();
        assert!(resolve(id, PromisePayload::Value(Vec::new(), None)));
        let result = rx.await.expect("receiver should receive");
        assert!(result.success);
        assert!(result.serialized_value.is_null());
        assert_eq!(result.serialized_value_len, 0);
    }

    #[tokio::test]
    async fn double_resolve_noops() {
        let (id, rx) = alloc();
        assert!(resolve(id, PromisePayload::Cancelled));
        assert!(
            !resolve(id, PromisePayload::Cancelled),
            "second resolve must be noop"
        );
        let _ = rx.await;
    }

    #[tokio::test]
    async fn cancel_delivers_exception() {
        let (id, rx) = alloc();
        assert!(cancel(id));
        let result = rx.await.unwrap();
        assert!(!result.success);
        assert_eq!(
            result.exception_class.as_deref(),
            Some("OxPHP\\Async\\AsyncException")
        );
        assert_eq!(
            result.exception_message.as_deref(),
            Some("synthetic promise cancelled")
        );
    }

    #[tokio::test]
    async fn reject_delivers_exception() {
        let (id, rx) = alloc();
        assert!(reject(id, "My\\Err", "boom"));
        let result = rx.await.unwrap();
        assert!(!result.success);
        assert_eq!(result.exception_class.as_deref(), Some("My\\Err"));
        assert_eq!(result.exception_message.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn resolve_value_delivers_and_returns_none() {
        let (id, rx) = alloc();
        assert!(resolve_value(id, vec![7, 8, 9], None).is_none());
        let result = rx.await.expect("receiver should receive");
        assert!(result.success);
        assert_eq!(result.serialized_value_len, 3);
    }

    #[tokio::test]
    async fn resolve_value_moves_keepalive_into_result() {
        let (id, rx) = alloc();
        let marker = Box::new(String::from("kept")) as Box<dyn std::any::Any + Send>;
        assert!(resolve_value(id, vec![9, 9], Some(marker)).is_none());
        let result = rx.await.unwrap();
        assert_eq!(result.serialized_value_len, 2);
        assert!(result.keepalive.is_some());
    }

    #[tokio::test]
    async fn resolve_value_hands_payload_back_when_already_resolved() {
        // Simulate the recvTimeout cancel winning the race: the promise
        // is resolved (sender removed) before resolve_value runs.
        let (id, rx) = alloc();
        assert!(cancel(id));
        // resolve_value must NOT drop the payload — it hands it back.
        let returned = resolve_value(id, vec![1, 2, 3], None);
        assert_eq!(returned.map(|(b, _)| b), Some(vec![1, 2, 3]));
        // The waiter saw the cancel, not the value.
        let result = rx.await.unwrap();
        assert!(!result.success);
        assert_eq!(
            result.exception_class.as_deref(),
            Some("OxPHP\\Async\\AsyncException")
        );
    }

    #[test]
    fn resolve_value_hands_back_when_receiver_dropped() {
        // Receiver torn down while the sender lingers (parked fiber gone).
        // resolve_value must hand the payload back, not consume it, so the
        // caller can try a live sibling or re-deposit it.
        let (id, rx) = alloc();
        drop(rx);
        assert_eq!(
            resolve_value(id, vec![5, 6], None).map(|(b, _)| b),
            Some(vec![5, 6])
        );
    }

    #[test]
    fn resolve_unknown_id_returns_false() {
        // Use an id that's guaranteed not to be in the map — synthetic
        // ids grow upward from i64::MIN + 1; i64::MAX is async-pool
        // territory and the map never sees it.
        assert!(!resolve(i64::MAX, PromisePayload::Cancelled));
    }
}
