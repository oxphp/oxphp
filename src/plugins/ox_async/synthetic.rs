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

/// Payload delivered to a suspended fiber.
#[derive(Debug)]
pub enum PromisePayload {
    /// Portable-serialised zval bytes (system-malloc'd buffer is built
    /// by this module, not by the caller). The receiver deserialises
    /// on its own thread.
    Value(Vec<u8>),
    /// Exception to throw: (fqn, message).
    Exception(String, String),
    /// Cancelled — no value; suspend site decides how to surface this
    /// (typically as a Closed/Stale error).
    Cancelled,
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
    let cancelled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
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
        PromisePayload::Value(bytes) => {
            if bytes.is_empty() {
                AsyncResult {
                    success: true,
                    serialized_value: std::ptr::null_mut(),
                    serialized_value_len: 0,
                    exception_class: None,
                    exception_message: None,
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
                }
            }
        }
        PromisePayload::Exception(class, message) => AsyncResult {
            success: false,
            serialized_value: std::ptr::null_mut(),
            serialized_value_len: 0,
            exception_class: Some(class),
            exception_message: Some(message),
        },
        PromisePayload::Cancelled => AsyncResult {
            success: false,
            serialized_value: std::ptr::null_mut(),
            serialized_value_len: 0,
            exception_class: Some("OxPHP\\Async\\AsyncException".to_string()),
            exception_message: Some("synthetic promise cancelled".to_string()),
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
        PromisePayload::Value(Vec::new())
    } else {
        let slice = unsafe { std::slice::from_raw_parts(payload_bytes, payload_len) };
        PromisePayload::Value(slice.to_vec())
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
        assert!(resolve(id, PromisePayload::Value(vec![1, 2, 3])));
        let result = rx.await.expect("receiver should receive");
        assert!(result.success);
        assert_eq!(result.serialized_value_len, 3);
        // Clean up the malloc'd buffer (AsyncResult::drop handles this).
    }

    #[tokio::test]
    async fn resolve_empty_value_uses_null_buf() {
        let (id, rx) = alloc();
        assert!(resolve(id, PromisePayload::Value(Vec::new())));
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

    #[test]
    fn resolve_unknown_id_returns_false() {
        // Use an id that's guaranteed not to be in the map — synthetic
        // ids grow upward from i64::MIN + 1; i64::MAX is async-pool
        // territory and the map never sees it.
        assert!(!resolve(i64::MAX, PromisePayload::Cancelled));
    }
}
