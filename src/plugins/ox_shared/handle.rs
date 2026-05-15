//! SharedHandle — per-instance storage attached to every Shared\*
//! PHP wrapper via ClassBuilder::with_storage().

use crate::plugins::ox_shared::registry::{Entry, SharedType};
use std::sync::Arc;

/// Stored inside every Shared\* PHP object via `with_storage`. The
/// wrapper has NO user-visible PHP properties — just this opaque slot.
///
/// `entry_ptr` is `Arc::into_raw(Arc<Entry>)`: the wrapper owns one
/// strong reference to the registry `Entry`. `Drop` reconstitutes the
/// `Arc` and drops it; when it is the last strong ref, `Entry::drop`
/// self-deregisters from the registry.
//
// #[repr(C)] keeps the field layout stable for C-side access from
// `oxphp_plugin_get_shared_handle` / `oxphp_shared_wrapper_new`, which
// read `entry_ptr` at offset 0 (pointer-width) and `type_tag` at
// offset 8 (u8). The tag-7 cross-thread serializer relies on this.
//
// Clone is preserved so ClassBuilder::with_storage_clone (used when
// `__clone` throws) still compiles; the clone path is never taken at
// runtime, but if it were it would `Arc::increment_strong_count`.
#[repr(C)]
#[derive(Debug)]
pub struct SharedHandle {
    pub entry_ptr: *const Entry, // offset 0 — NULL means "uninitialised wrapper"
    pub type_tag: u8,            // offset 8
}

// SAFETY: `entry_ptr` is `Arc::into_raw` of an `Arc<Entry>`, and
// `Entry: Send + Sync`. The handle owns exactly one strong ref, so
// moving it between threads is sound — identical to moving an
// `Arc<Entry>`.
unsafe impl Send for SharedHandle {}
unsafe impl Sync for SharedHandle {}

impl SharedHandle {
    pub fn new(type_tag: SharedType) -> Self {
        Self {
            entry_ptr: std::ptr::null(), // populated by __construct
            type_tag: type_tag as u8,
        }
    }

    pub fn is_initialized(&self) -> bool {
        !self.entry_ptr.is_null()
    }
}

impl Clone for SharedHandle {
    fn clone(&self) -> Self {
        if !self.entry_ptr.is_null() {
            // SAFETY: entry_ptr is a live Arc::into_raw pointer.
            unsafe { Arc::increment_strong_count(self.entry_ptr) };
        }
        Self {
            entry_ptr: self.entry_ptr,
            type_tag: self.type_tag,
        }
    }
}

impl Drop for SharedHandle {
    fn drop(&mut self) {
        if !self.entry_ptr.is_null() {
            // SAFETY: entry_ptr came from Arc::into_raw in the type
            // constructor (or oxphp_shared_handle_from_id). Reconstitute
            // and drop exactly once — this handle owns one strong ref.
            unsafe { drop(Arc::from_raw(self.entry_ptr)) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_starts_uninitialised() {
        let h = SharedHandle::new(SharedType::Counter);
        assert!(h.entry_ptr.is_null());
        assert!(!h.is_initialized());
        assert_eq!(h.type_tag, SharedType::Counter as u8);
    }
}
