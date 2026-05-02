//! SharedRegistry — process-global entry store. Arc-refcount lifecycle.
//!
//! Spec: .internal/technical-docs/en/features/shared/01-registry.md

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use dashmap::DashMap;

use crate::plugins::ox_shared::config::SharedConfig;
use crate::plugins::ox_shared::error::{set_last_error, SharedError};
use crate::plugins::ox_shared::value::SharedValue;

pub type SharedId = u64;

#[repr(u8)]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum SharedType {
    Counter = 10,
    Flag = 11,
    Once = 12,
    // Reserved — later phases.
    Mutex = 40,
    Channel = 31,
    Map = 20,
    Pool = 50,
}

impl SharedType {
    pub fn from_tag(tag: u8) -> Option<Self> {
        match tag {
            10 => Some(Self::Counter),
            11 => Some(Self::Flag),
            12 => Some(Self::Once),
            40 => Some(Self::Mutex),
            31 => Some(Self::Channel),
            20 => Some(Self::Map),
            50 => Some(Self::Pool),
            _ => None,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Counter => "Counter",
            Self::Flag => "Flag",
            Self::Once => "Once",
            Self::Mutex => "Mutex",
            Self::Channel => "Channel",
            Self::Map => "Map",
            Self::Pool => "Pool",
        }
    }

    pub fn php_class(&self) -> &'static str {
        match self {
            Self::Counter => "OxPHP\\Shared\\Counter",
            Self::Flag => "OxPHP\\Shared\\Flag",
            Self::Once => "OxPHP\\Shared\\Once",
            Self::Mutex => "OxPHP\\Shared\\Mutex",
            Self::Channel => "OxPHP\\Shared\\Channel",
            Self::Map => "OxPHP\\Shared\\Map",
            Self::Pool => "OxPHP\\Shared\\Pool",
        }
    }

    /// NUL-terminated FQN, suitable for direct use as a `*const c_char`
    /// from C without an intermediate copy. Backs `oxphp_shared_class_name`,
    /// which the C bridge calls during cross-thread (tag-7) deserialization
    /// instead of duplicating the tag→class mapping.
    pub fn php_class_cstr(&self) -> &'static std::ffi::CStr {
        match self {
            Self::Counter => c"OxPHP\\Shared\\Counter",
            Self::Flag => c"OxPHP\\Shared\\Flag",
            Self::Once => c"OxPHP\\Shared\\Once",
            Self::Mutex => c"OxPHP\\Shared\\Mutex",
            Self::Channel => c"OxPHP\\Shared\\Channel",
            Self::Map => c"OxPHP\\Shared\\Map",
            Self::Pool => c"OxPHP\\Shared\\Pool",
        }
    }
}

/// Each `Entry` in the registry is explicitly reference-counted via
/// `ext_refcount`. `insert` seeds it at 1 (for the creating wrapper);
/// `retain`/`release` atomically bump/drop it. When it reaches 0 the
/// entry is evicted from the DashMap. Arc<Entry> clones from lookup()
/// provide short-lived strong refs during FFI calls but are NOT the
/// primary lifetime anchor — without explicit retain, an entry can be
/// freed as soon as all wrapper Drops run.
pub struct Entry {
    pub id: SharedId,
    pub type_tag: SharedType,
    pub inner: Arc<dyn SharedInner>,
    pub created_at: Instant,
    pub ops: AtomicU64,
    pub mem_bytes: AtomicUsize,
    pub ext_refcount: AtomicU64,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("id", &self.id)
            .field("type_tag", &self.type_tag)
            .field("ops", &self.ops.load(Ordering::Relaxed))
            .field("mem_bytes", &self.mem_bytes.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

pub trait SharedInner: Send + Sync + 'static {
    fn type_tag(&self) -> SharedType;
    fn debug_snapshot(&self) -> SharedValue;
    fn mem_bytes(&self) -> usize;
    fn on_drop(&self) {}
    fn on_shutdown_notify(&self) {}

    /// Enumerate outgoing [`SharedRef`] edges for the cycle walker.
    /// Default: no outgoing edges — correct for scalar containers
    /// (Counter, Flag) and for container types that do not nest
    /// `SharedValue::Shared` in v1 (Once, Mutex, Channel).
    ///
    /// Called from [`crate::plugins::ox_shared::cycle::would_create_cycle`]
    /// during a Map::set pre-check.
    fn children(&self, _out: &mut Vec<crate::plugins::ox_shared::value::SharedRef>) {}
}

pub struct SharedRegistry {
    entries: DashMap<SharedId, Arc<Entry>>,
    next_id: AtomicI64,
    total_bytes: AtomicU64,
    total_entries: AtomicU64,
    config: SharedConfig,
    shutting_down: AtomicBool,
}

pub(crate) static REGISTRY: OnceLock<SharedRegistry> = OnceLock::new();

pub fn registry() -> &'static SharedRegistry {
    REGISTRY
        .get()
        .expect("SharedRegistry not initialised — call init_registry first")
}

pub fn init_registry(config: SharedConfig) {
    let reg = SharedRegistry {
        entries: DashMap::with_capacity(128),
        next_id: AtomicI64::new(1),
        total_bytes: AtomicU64::new(0),
        total_entries: AtomicU64::new(0),
        config,
        shutting_down: AtomicBool::new(false),
    };
    REGISTRY.set(reg).ok();
}

impl SharedRegistry {
    pub fn config(&self) -> &SharedConfig {
        &self.config
    }

    pub fn total_entries(&self) -> u64 {
        self.total_entries.load(Ordering::Relaxed)
    }

    pub fn total_bytes(&self) -> u64 {
        self.total_bytes.load(Ordering::Relaxed)
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    /// Insert a new entry. Returns the SharedId.
    /// Fails with CapacityExceeded if hard caps would be breached.
    pub fn insert(
        &self,
        type_tag: SharedType,
        inner: Arc<dyn SharedInner>,
    ) -> Result<SharedId, SharedError> {
        let mem = inner.mem_bytes();

        // Hard-cap check on entries count.
        let new_count = self.total_entries.load(Ordering::Relaxed) + 1;
        if new_count as usize > self.config.max_entries {
            set_last_error(format!(
                "Entries capacity exceeded: {} / {} entries",
                new_count, self.config.max_entries
            ));
            return Err(SharedError::CapacityExceeded);
        }

        // Hard-cap check on total bytes.
        let new_bytes = self.total_bytes.load(Ordering::Relaxed) + mem as u64;
        if new_bytes > self.config.max_bytes {
            set_last_error(format!(
                "Bytes capacity exceeded: {} / {} bytes",
                new_bytes, self.config.max_bytes
            ));
            return Err(SharedError::CapacityExceeded);
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed) as u64;
        let entry = Arc::new(Entry {
            id,
            type_tag,
            inner,
            created_at: Instant::now(),
            ops: AtomicU64::new(0),
            mem_bytes: AtomicUsize::new(mem),
            // Seeded at 1 for the creating wrapper; its Drop will call release().
            ext_refcount: AtomicU64::new(1),
        });
        self.entries.insert(id, entry);
        self.total_entries.fetch_add(1, Ordering::Relaxed);
        self.total_bytes.fetch_add(mem as u64, Ordering::Relaxed);
        Ok(id)
    }

    /// Look up an entry; returns StaleHandle if missing.
    pub fn lookup(&self, id: SharedId) -> Result<Arc<Entry>, SharedError> {
        self.entries
            .get(&id)
            .map(|r| Arc::clone(&r))
            .ok_or(SharedError::StaleHandle)
    }

    /// Record an op (increments per-entry counter).
    pub fn record_op(&self, id: SharedId) {
        if let Some(e) = self.entries.get(&id) {
            e.ops.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Retain: atomically increment the entry's ext_refcount. Returns
    /// the new count, or -1 if the entry does not exist.
    pub fn retain(&self, id: SharedId) -> i32 {
        match self.entries.get(&id) {
            Some(e) => (e.ext_refcount.fetch_add(1, Ordering::AcqRel) + 1) as i32,
            None => -1,
        }
    }

    /// Release: atomically decrement ext_refcount. If it reaches 0 the
    /// entry is evicted from the registry (on_drop fires, totals adjust).
    /// Returns the new count, or -1 if the entry does not exist.
    pub fn release(&self, id: SharedId) -> i32 {
        let prev = match self.entries.get(&id) {
            Some(e) => e.ext_refcount.fetch_sub(1, Ordering::AcqRel),
            None => return -1,
        };
        if prev == 1 {
            if let Some((_, entry)) = self.entries.remove(&id) {
                entry.inner.on_drop();
                self.total_entries.fetch_sub(1, Ordering::Relaxed);
                self.total_bytes.fetch_sub(
                    entry.mem_bytes.load(Ordering::Relaxed) as u64,
                    Ordering::Relaxed,
                );
            }
            return 0;
        }
        (prev - 1) as i32
    }

    pub fn iter_entries(&self) -> impl Iterator<Item = Arc<Entry>> + '_ {
        self.entries.iter().map(|r| Arc::clone(r.value()))
    }

    pub fn is_alive(&self, id: SharedId) -> bool {
        self.entries.contains_key(&id)
    }

    pub fn type_tag(&self, id: SharedId) -> Option<SharedType> {
        self.entries.get(&id).map(|e| e.type_tag)
    }

    /// Peek at the external refcount of an entry. Returns `None` if the
    /// entry does not exist. Used by tests that verify retain/release
    /// balance; not a contract surface.
    pub fn ext_refcount(&self, id: SharedId) -> Option<u64> {
        self.entries
            .get(&id)
            .map(|e| e.ext_refcount.load(Ordering::Acquire))
    }

    /// Drain: wake blocked ops, mark shutting-down.
    /// No-op for atomic types because they don't block.
    pub fn drain(&self) {
        self.shutting_down.store(true, Ordering::Release);
        for e in self.entries.iter() {
            e.inner.on_shutdown_notify();
        }
    }
}

// ─── FFI ──────────────────────────────────────────────────────────────

use crate::plugins::ox_shared::error::ffi_entry;
use std::os::raw::{c_char, c_int};

#[no_mangle]
pub extern "C" fn oxphp_shared_registry_init() -> c_int {
    ffi_entry(|| {
        if REGISTRY.get().is_none() {
            set_last_error(
                "registry not yet initialised from Rust; SharedPlugin::init must run first",
            );
            return Err(SharedError::Generic);
        }
        Ok(())
    })
}

#[no_mangle]
pub extern "C" fn oxphp_shared_retain(id: u64) -> c_int {
    ffi_entry(|| {
        if let Some(reg) = REGISTRY.get() {
            Ok::<_, SharedError>(reg.retain(id))
        } else {
            Err(SharedError::Generic)
        }
        .map(|_| ())
    })
}

#[no_mangle]
pub extern "C" fn oxphp_shared_release(id: u64) -> c_int {
    ffi_entry(|| {
        if let Some(reg) = REGISTRY.get() {
            reg.release(id);
            Ok(())
        } else {
            Err(SharedError::Generic)
        }
    })
}

#[no_mangle]
pub extern "C" fn oxphp_shared_is_alive(id: u64) -> c_int {
    match REGISTRY.get() {
        Some(reg) => reg.is_alive(id) as c_int,
        None => 0,
    }
}

/// Returns the NUL-terminated PHP class FQN for a `SharedType` tag, or
/// `NULL` for an unknown tag. The pointer is to a `&'static CStr` and is
/// valid for the lifetime of the process — caller must NOT free it.
///
/// Called by the C bridge from `oxphp_shared_wrapper_new` during tag-7
/// (cross-thread Shareable) deserialization. Single source of truth for
/// the tag→FQN mapping — the C side calls this instead of duplicating
/// the switch.
#[no_mangle]
pub extern "C" fn oxphp_shared_class_name(type_tag: u8) -> *const c_char {
    match SharedType::from_tag(type_tag) {
        Some(t) => t.php_class_cstr().as_ptr(),
        None => std::ptr::null(),
    }
}

#[no_mangle]
pub extern "C" fn oxphp_shared_type_tag(id: u64) -> u8 {
    REGISTRY
        .get()
        .and_then(|r| r.type_tag(id))
        .map(|t| t as u8)
        .unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn oxphp_shared_total_entries() -> u64 {
    REGISTRY.get().map(|r| r.total_entries()).unwrap_or(0)
}

#[no_mangle]
pub extern "C" fn oxphp_shared_total_bytes() -> u64 {
    REGISTRY.get().map(|r| r.total_bytes()).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::ox_shared::value::SharedValue;

    struct TestInner {
        bytes: usize,
    }

    impl SharedInner for TestInner {
        fn type_tag(&self) -> SharedType {
            SharedType::Counter
        }
        fn debug_snapshot(&self) -> SharedValue {
            SharedValue::Null
        }
        fn mem_bytes(&self) -> usize {
            self.bytes
        }
    }

    fn fresh_registry() -> &'static SharedRegistry {
        let cfg = SharedConfig {
            enabled: true,
            max_entries: 100,
            max_bytes: 1024,
            soft_limit_ratio: 0.7,
            metrics_enabled: true,
            introspection_enabled: true,
            introspection_preview_enabled: true,
            cycle_detect_depth: 16,
            cycle_detect_edges: 10000,
            shutdown_timeout_seconds: 5.0,
            poison_strict: false,
            lock_diagnostics: crate::plugins::ox_shared::config::LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
            preview_string_limit: 256,
            preview_array_limit: 20,
        };
        init_registry(cfg);
        registry()
    }

    #[test]
    fn insert_and_lookup() {
        let reg = fresh_registry();
        let id = reg
            .insert(SharedType::Counter, Arc::new(TestInner { bytes: 16 }))
            .unwrap();
        assert!(reg.is_alive(id));
        assert_eq!(reg.type_tag(id), Some(SharedType::Counter));
        let e = reg.lookup(id).unwrap();
        assert_eq!(e.type_tag, SharedType::Counter);
    }

    #[test]
    fn stale_lookup_errors() {
        let reg = fresh_registry();
        let err = reg.lookup(99999).unwrap_err();
        assert_eq!(err, SharedError::StaleHandle);
    }

    #[test]
    fn total_counts_track() {
        let reg = fresh_registry();
        let before = reg.total_entries();
        let _id = reg
            .insert(SharedType::Counter, Arc::new(TestInner { bytes: 16 }))
            .unwrap();
        assert_eq!(reg.total_entries(), before + 1);
    }

    #[test]
    fn shared_type_roundtrip() {
        for tag in [SharedType::Counter, SharedType::Flag, SharedType::Once] {
            let byte = tag as u8;
            assert_eq!(SharedType::from_tag(byte), Some(tag));
        }
    }

    /// Pins the contract `oxphp_shared_wrapper_new` (C bridge) relies on
    /// when rebuilding a Shareable wrapper on a worker thread: every
    /// `SharedType` variant must map to a non-null FQN.
    #[test]
    fn class_name_ffi_covers_every_shared_type() {
        use std::ffi::CStr;
        let cases = [
            (SharedType::Counter, "OxPHP\\Shared\\Counter"),
            (SharedType::Flag, "OxPHP\\Shared\\Flag"),
            (SharedType::Once, "OxPHP\\Shared\\Once"),
            (SharedType::Mutex, "OxPHP\\Shared\\Mutex"),
            (SharedType::Channel, "OxPHP\\Shared\\Channel"),
            (SharedType::Map, "OxPHP\\Shared\\Map"),
            (SharedType::Pool, "OxPHP\\Shared\\Pool"),
        ];
        for (ty, expected_fqn) in cases {
            let ptr = oxphp_shared_class_name(ty as u8);
            assert!(
                !ptr.is_null(),
                "tag {} ({:?}) not handled by oxphp_shared_class_name",
                ty as u8,
                ty
            );
            let actual = unsafe { CStr::from_ptr(ptr) }
                .to_str()
                .expect("FQN must be valid UTF-8");
            assert_eq!(actual, expected_fqn, "FQN mismatch for {:?}", ty);
        }
    }

    #[test]
    fn class_name_ffi_returns_null_for_unknown_tag() {
        for tag in [0u8, 1, 99, 200, 255] {
            assert!(
                SharedType::from_tag(tag).is_none(),
                "tag {tag} unexpectedly known"
            );
            assert!(
                oxphp_shared_class_name(tag).is_null(),
                "unknown tag {tag} must map to NULL"
            );
        }
    }
}
