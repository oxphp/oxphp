// Async promise types for OxPHP async worker pool.
//
// These structs define the data exchanged between PHP workers (which freeze
// closures and capture variables) and the async Tokio worker pool (which
// executes the closures and returns results).

use std::ffi::c_void;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

/// A task dispatched from a PHP worker thread to the async worker pool.
///
/// Contains all the data needed to execute a PHP closure on a different thread:
/// a thread-local copy of the zend_op_array struct, portable-serialized static
/// variables, an optional borrowed `$this` reference, and portable-serialized
/// argument data.
pub struct AsyncTask {
    /// Unique identifier for the promise this task belongs to.
    pub promise_id: u64,
    /// System-malloc'd copy of the `zend_op_array` struct bytes.
    /// Copied on the dispatching thread to avoid cross-thread reads.
    /// The internal pointers (opcodes, literals) reference OPcache SHM or
    /// stable compiled-script memory — safe to dereference from any thread.
    pub op_array_buf: *mut u8,
    /// Size of the op_array_buf (sizeof(zend_op_array)).
    pub op_array_buf_len: usize,
    /// Portable-serialized static variables buffer (system malloc'd, thread-safe).
    /// Null if the closure has no use-vars.
    pub serialized_static_vars: *mut u8,
    /// Length of the serialized_static_vars buffer.
    pub serialized_static_vars_len: usize,
    /// Borrowed `$this` object pointer, or null if the closure is not bound.
    pub this_ptr: *mut c_void,
    /// Number of arguments (used as count hint for deserialization).
    pub argc: u32,
    /// Portable-serialized argument buffer (system malloc'd, thread-safe).
    /// Null if argc == 0.
    pub serialized_args: *mut u8,
    /// Length of the serialized_args buffer.
    pub serialized_args_len: usize,
    /// Shared cancellation flag — checked by the async worker before/during execution.
    pub cancelled: Arc<AtomicBool>,
    /// One-shot channel to send the result back to the originating PHP worker.
    pub result_tx: tokio::sync::oneshot::Sender<AsyncResult>,
}

// SAFETY: op_array_buf is a system-malloc'd buffer exclusively owned by this task,
// containing a snapshot of the zend_op_array struct. Its internal pointers reference
// OPcache SHM or stable compiled-script memory (safe from any thread).
// Serialization buffers are system-malloc'd and exclusively owned.
// The Arc<AtomicBool> is inherently thread-safe. The oneshot::Sender is Send.
unsafe impl Send for AsyncTask {}

/// The result of executing an async task, sent back to the originating PHP worker.
pub struct AsyncResult {
    /// Whether the closure executed successfully (true) or threw an exception (false).
    pub success: bool,
    /// Portable-serialized return value buffer (system malloc'd, thread-safe).
    /// Null on failure or void return.
    pub serialized_value: *mut u8,
    /// Length of the serialized_value buffer.
    pub serialized_value_len: usize,
    /// Exception class name, if the closure threw.
    pub exception_class: Option<String>,
    /// Exception message, if the closure threw.
    pub exception_message: Option<String>,
    /// Type-erased keepalive that pins nested `Shared\*` entries alive until
    /// the receiving fiber deserializes `serialized_value`. The concrete type
    /// is owned by the channel layer (`SmallVec<[SharedRefOwned; 1]>`); this
    /// layer only holds it so it drops at the right time — at the end of
    /// `await_dispatch_callback`, after `oxphp_portable_deserialize`. `None`
    /// for every non-channel result and the common no-shared-ref case.
    pub keepalive: Option<Box<dyn std::any::Any + Send>>,
}

// SAFETY: The serialized_value pointer is a system-malloc'd buffer exclusively
// owned by this result — no other thread holds a reference to it. String fields are owned.
unsafe impl Send for AsyncResult {}

impl std::fmt::Debug for AsyncResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AsyncResult")
            .field("success", &self.success)
            .field("serialized_value_len", &self.serialized_value_len)
            .field("exception_class", &self.exception_class)
            .field("exception_message", &self.exception_message)
            .field("keepalive", &self.keepalive.is_some())
            .finish()
    }
}

impl Drop for AsyncResult {
    fn drop(&mut self) {
        if !self.serialized_value.is_null() {
            // System malloc'd buffer — safe to free from any thread.
            unsafe { libc::free(self.serialized_value as *mut c_void) };
            self.serialized_value = std::ptr::null_mut();
        }
    }
}

/// Tracks a frozen zval so it can be unfrozen (restored) after the async task completes.
///
/// Freezing a zval makes it read-only by saving and clearing its refcount and GC flags.
/// This struct records the original values so they can be restored during cleanup.
pub struct FrozenZval {
    /// Pointer to the zval that was frozen.
    pub zval_ptr: *mut c_void,
    /// Original `zend_refcounted_h.refcount` before freeze.
    pub orig_refcount: u32,
    /// Original `zend_refcounted_h.gc_flags` before freeze.
    pub orig_gc_flags: u32,
    /// Original `zend_refcounted_h.type_flags` before freeze (GC type info).
    pub orig_type_flags: u32,
}

// SAFETY: The zval_ptr points to a zval on the originating thread's heap.
// FrozenZval is only sent back to that same thread for cleanup, so the pointer
// remains valid. Between freeze and unfreeze, no thread mutates the zval.
unsafe impl Send for FrozenZval {}

/// Tracks a borrowed object whose zval was replaced by a proxy on the originating thread.
///
/// The original zval bytes are backed up so the zval can be restored after the
/// async task completes.
pub struct BorrowedZval {
    /// Pointer to the proxy zval that replaced the original.
    pub proxy_zval_ptr: *mut c_void,
    /// Raw backup of the original 16-byte zval content (value + type tag).
    pub original_zval_data: [u8; 16],
}

// SAFETY: Similar to FrozenZval — the proxy_zval_ptr belongs to the originating
// thread and is only used for restoration on that same thread. The byte backup
// is a plain Copy value with no pointers of its own.
unsafe impl Send for BorrowedZval {}

/// Per-promise cleanup state that tracks all frozen and borrowed zvals.
///
/// After an async task completes (or is cancelled), this struct is used to
/// restore all modified zvals to their original state on the originating thread.
pub struct PromiseCleanup {
    /// Zvals that were frozen (made read-only) for this promise.
    pub frozen: Vec<FrozenZval>,
    /// Zvals that were borrowed (replaced by proxies) for this promise.
    pub borrowed: Vec<BorrowedZval>,
    /// Closure zval pointer (addref'd to prevent GC while async task holds op_array).
    /// Must be dtor'd on cleanup to release the reference.
    pub closure_zval: *mut std::ffi::c_void,
}

// SAFETY: Contains only FrozenZval and BorrowedZval (both Send). Only accessed
// from the originating thread for cleanup after async task completion.
unsafe impl Send for PromiseCleanup {}

impl PromiseCleanup {
    /// Creates a new empty cleanup tracker.
    pub fn new() -> Self {
        Self {
            frozen: Vec::new(),
            borrowed: Vec::new(),
            closure_zval: std::ptr::null_mut(),
        }
    }
}

impl Default for PromiseCleanup {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    #[test]
    fn test_async_task_creation() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let (tx, _rx) = tokio::sync::oneshot::channel::<AsyncResult>();

        let task = AsyncTask {
            promise_id: 42,
            op_array_buf: ptr::null_mut(),
            op_array_buf_len: 0,
            serialized_static_vars: ptr::null_mut(),
            serialized_static_vars_len: 0,
            this_ptr: ptr::null_mut(),
            argc: 3,
            serialized_args: ptr::null_mut(),
            serialized_args_len: 0,
            cancelled: cancelled.clone(),
            result_tx: tx,
        };

        assert_eq!(task.promise_id, 42);
        assert!(task.op_array_buf.is_null());
        assert!(task.serialized_static_vars.is_null());
        assert!(task.this_ptr.is_null());
        assert_eq!(task.argc, 3);
        assert!(task.serialized_args.is_null());
        assert!(!task.cancelled.load(Ordering::Relaxed));
    }

    #[test]
    fn test_async_result_success() {
        let result = AsyncResult {
            success: true,
            serialized_value: ptr::null_mut(),
            serialized_value_len: 0,
            exception_class: None,
            exception_message: None,
            keepalive: None,
        };

        assert!(result.success);
        assert!(result.serialized_value.is_null());
        assert!(result.exception_class.is_none());
        assert!(result.exception_message.is_none());
    }

    #[test]
    fn test_async_result_failure() {
        let result = AsyncResult {
            success: false,
            serialized_value: ptr::null_mut(),
            serialized_value_len: 0,
            exception_class: Some("RuntimeException".to_string()),
            exception_message: Some("something went wrong".to_string()),
            keepalive: None,
        };

        assert!(!result.success);
        assert!(result.serialized_value.is_null());
        assert_eq!(result.exception_class.as_deref(), Some("RuntimeException"));
        assert_eq!(
            result.exception_message.as_deref(),
            Some("something went wrong")
        );
    }

    #[test]
    fn test_frozen_zval_tracking() {
        let frozen = FrozenZval {
            zval_ptr: ptr::null_mut(),
            orig_refcount: 5,
            orig_gc_flags: 0x01,
            orig_type_flags: 0x02,
        };

        assert!(frozen.zval_ptr.is_null());
        assert_eq!(frozen.orig_refcount, 5);
        assert_eq!(frozen.orig_gc_flags, 0x01);
        assert_eq!(frozen.orig_type_flags, 0x02);
    }

    #[test]
    fn test_borrowed_zval_tracking() {
        let borrowed = BorrowedZval {
            proxy_zval_ptr: ptr::null_mut(),
            original_zval_data: [0xAB; 16],
        };

        assert!(borrowed.proxy_zval_ptr.is_null());
        assert_eq!(borrowed.original_zval_data, [0xAB; 16]);
    }

    #[test]
    fn test_promise_cleanup_empty() {
        let cleanup = PromiseCleanup::new();
        assert!(cleanup.frozen.is_empty());
        assert!(cleanup.borrowed.is_empty());
    }

    #[test]
    fn test_promise_cleanup_add_frozen() {
        let mut cleanup = PromiseCleanup::new();
        cleanup.frozen.push(FrozenZval {
            zval_ptr: ptr::null_mut(),
            orig_refcount: 1,
            orig_gc_flags: 0,
            orig_type_flags: 0,
        });
        assert_eq!(cleanup.frozen.len(), 1);
    }

    #[test]
    fn test_async_task_send_trait() {
        fn assert_send<T: Send>() {}
        assert_send::<AsyncTask>();
    }

    #[test]
    fn test_async_result_send_trait() {
        fn assert_send<T: Send>() {}
        assert_send::<AsyncResult>();
    }
}
