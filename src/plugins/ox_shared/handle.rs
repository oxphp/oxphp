//! SharedHandle — per-instance storage attached to every Shared\*
//! PHP wrapper via ClassBuilder::with_storage().
//!
//! Spec: .internal/technical-docs/en/features/shared/01-registry.md
//!       §PHP-side wrapper

use crate::plugins::ox_shared::registry::SharedType;

/// Stored inside every Shared\* PHP object via with_storage. The
/// wrapper has NO user-visible PHP properties — just this opaque slot.
// NOTE: Copy is incompatible with Drop. Clone is preserved so
// ClassBuilder::with_storage_clone (used when `__clone` throws — Part D/E/F)
// can still compile against the handle, though the clone path is never
// actually taken at runtime.
//
// #[repr(C)] guarantees the field layout is stable for C-side access from
// `oxphp_plugin_get_shared_handle` / `oxphp_shared_wrapper_new`, which
// read shared_id at offset 0 (u64) and type_tag at offset 8 (u8). The
// tag-7 cross-thread serializer relies on this.
#[repr(C)]
#[derive(Debug, Clone)]
pub struct SharedHandle {
    pub shared_id: u64, // offset 0, 8 bytes — 0 means "uninitialised wrapper"
    pub type_tag: u8,   // offset 8, 1 byte
}

impl SharedHandle {
    pub fn new(type_tag: SharedType) -> Self {
        Self {
            shared_id: 0, // populated by __construct
            type_tag: type_tag as u8,
        }
    }

    pub fn is_initialized(&self) -> bool {
        self.shared_id != 0
    }
}

impl Drop for SharedHandle {
    fn drop(&mut self) {
        // When the PHP wrapper is freed, decrement the registry refcount.
        // This is the only place `release` is called from the Rust-owned
        // wrapper side; external C code MAY also call it for manual refcount
        // management, but the common path runs here.
        if self.shared_id != 0 {
            if let Some(reg) = crate::plugins::ox_shared::registry::REGISTRY.get() {
                reg.release(self.shared_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handle_starts_uninitialised() {
        let h = SharedHandle::new(SharedType::Counter);
        assert_eq!(h.shared_id, 0);
        assert!(!h.is_initialized());
        assert_eq!(h.type_tag, SharedType::Counter as u8);
    }
}
