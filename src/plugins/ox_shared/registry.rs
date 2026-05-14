//! SharedRegistry — process-global entry store. Arc-refcount lifecycle.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use dashmap::DashMap;

use crate::plugins::ox_shared::config::SharedConfig;
use crate::plugins::ox_shared::error::{set_last_error, SharedError};
use crate::plugins::ox_shared::value::SharedValue;

pub type SharedId = u64;

/// Per-entry fixed overhead booked against `total_bytes` at insert,
/// in addition to `SharedInner::mem_bytes()`. Accounts for the storage
/// chain around the inner value:
///
/// | Component                                  | Bytes (approx) |
/// |--------------------------------------------|----------------|
/// | `Entry` struct                             | ~72            |
/// | `Arc<Entry>` strong/weak header            | 16             |
/// | `Arc<dyn SharedInner>` header              | 16             |
/// | DashMap shard bucket entry                 | 32–48          |
/// | Per-allocation malloc prologue (×3 allocs) | ~48            |
/// | **Total**                                  | **184–216**    |
///
/// 200 sits in the middle of that structural breakdown. **The number
/// is a static accounting estimate, NOT measured against a heap
/// profiler** — calibrating against heaptrack / `jemalloc_stats_print`
/// / `mi_stats_print` on a target platform (glibc, musl, macOS) is a
/// nice-to-have follow-up that would tighten the bound, but adds CI
/// dependencies. For now the doc and the code are honest about being
/// a guess inside a known range.
///
/// Treat `max_bytes` as a grace-cap, not a precise RSS budget. Operators
/// should still cap the container at the orchestrator level (cgroups,
/// k8s `resources.limits.memory`) for hard isolation.
///
/// **Capacity-planning impact for operators on upgrade**: this constant
/// did not exist before — scalar types (`Atomic`, `Counter`, `Flag`)
/// reported only their ~16 B content. After this change the same
/// `OX_SHARED_MAX_BYTES` admits roughly an order of magnitude fewer
/// scalar entries (200 B booked per insert instead of ~16 B). See the
/// `CHANGELOG.md` Migration section for the recommended cap retune.
pub const ENTRY_FIXED_OVERHEAD: usize = 200;

#[repr(u8)]
#[derive(Copy, Clone, Eq, PartialEq, Debug, Hash)]
pub enum SharedType {
    Counter = 10,
    Flag = 11,
    Once = 12,
    Atomic = 13,
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
            13 => Some(Self::Atomic),
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
            Self::Atomic => "Atomic",
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
            Self::Atomic => "OxPHP\\Shared\\Atomic",
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
            Self::Atomic => c"OxPHP\\Shared\\Atomic",
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

pub trait SharedInner: std::any::Any + Send + Sync + 'static {
    fn type_tag(&self) -> SharedType;

    /// Upcast to `&dyn Any` so callers can `downcast_ref` to the concrete
    /// `*Inner` type. This is the soundness-safe replacement for casting the
    /// `dyn SharedInner` fat pointer to a thin `*const ConcreteInner`: a
    /// `TypeId` mismatch yields `None` instead of a read at a wrong offset.
    fn as_any(&self) -> &dyn std::any::Any;

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

    /// Registry-wide sum of every entry's `mem_bytes`.
    ///
    /// This is shared mutable state: under a parallel test run, entries
    /// created by *other* tests mutate it concurrently. A test that
    /// needs an exact byte delta from a single operation must therefore
    /// read the relevant `Entry::mem_bytes` instead — that counter is
    /// touched only by the entity owning the id, so the delta is
    /// deterministic. See the `*_track_registry_entry_bytes` tests in
    /// the type modules for the pattern.
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes.load(Ordering::Relaxed)
    }

    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.load(Ordering::Acquire)
    }

    /// Insert a new entry. Returns the SharedId.
    /// Fails with CapacityExceeded if hard caps would be breached.
    ///
    /// Reserves capacity via `fetch_add` and rolls back on cap-miss
    /// (Tokio-style semaphore pattern). A concurrent burst may briefly
    /// observe `total_*` above the configured cap (for the duration of
    /// the failing thread's rollback — sub-µs); the post-condition
    /// `total_* <= max_*` is preserved.
    pub fn insert(
        &self,
        type_tag: SharedType,
        inner: Arc<dyn SharedInner>,
    ) -> Result<SharedId, SharedError> {
        // The caller passes `type_tag` separately from `inner`; they must
        // agree, otherwise downcasts via `as_any().downcast_ref()` would
        // silently return `None` for a value that is actually present.
        debug_assert_eq!(
            type_tag,
            inner.type_tag(),
            "type_tag mismatch: caller passed {:?}, inner reports {:?}",
            type_tag,
            inner.type_tag()
        );

        // Inner content + per-entry overhead (storage chain around the
        // inner value — see ENTRY_FIXED_OVERHEAD doc). Saturating-add
        // keeps the arithmetic robust against a pathological inner that
        // reports near-`usize::MAX`; in practice inner.mem_bytes() is
        // bounded by per-instance caps inside each type.
        let mem = inner.mem_bytes().saturating_add(ENTRY_FIXED_OVERHEAD);

        // Reserve an entries slot. Relaxed: `total_entries` is a pure
        // accumulator for the cap-check — it establishes no happens-before
        // with the inserted Entry. The Entry becomes visible to other
        // threads through `self.entries.insert` below, whose DashMap shard
        // lock carries its own ordering.
        let new_count = self.total_entries.fetch_add(1, Ordering::Relaxed) + 1;
        if new_count as usize > self.config.max_entries {
            self.total_entries.fetch_sub(1, Ordering::Relaxed);
            set_last_error(format!(
                "Entries capacity exceeded: {} / {} entries",
                new_count, self.config.max_entries
            ));
            return Err(SharedError::CapacityExceeded);
        }

        // Reserve bytes.
        let new_bytes = self.total_bytes.fetch_add(mem as u64, Ordering::Relaxed) + mem as u64;
        if new_bytes > self.config.max_bytes {
            self.total_bytes.fetch_sub(mem as u64, Ordering::Relaxed);
            self.total_entries.fetch_sub(1, Ordering::Relaxed);
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
        Ok(id)
    }

    /// Look up an entry; returns StaleHandle if missing.
    pub fn lookup(&self, id: SharedId) -> Result<Arc<Entry>, SharedError> {
        self.entries
            .get(&id)
            .map(|r| Arc::clone(&r))
            .ok_or(SharedError::StaleHandle)
    }

    /// Record a single operation on `entry`. No-op when
    /// `config.metrics_enabled` is `false`.
    ///
    /// Takes `&Entry` (not `SharedId`) so the caller's existing
    /// `lookup()` result is reused — this avoids a second DashMap
    /// shard-lock per call.
    pub fn record_op(&self, entry: &Entry) {
        if !self.config.metrics_enabled {
            return;
        }
        // Relaxed: `ops` is a pure metric counter, never read for
        // synchronisation — unlike `ext_refcount`, which is a lifecycle
        // guard and therefore uses AcqRel.
        entry.ops.fetch_add(1, Ordering::Relaxed);
    }

    /// Adjust a live entry's accounted memory by `delta` bytes.
    ///
    /// Called by container types (Map, Channel, Pool) whose internal
    /// footprint changes after `insert` — without this, `total_bytes`
    /// reflects only the initial inner size and drifts arbitrarily far
    /// from reality as the container grows. Positive `delta` for adds,
    /// negative for removes; `0` is a no-op.
    ///
    /// Best-effort, never fails:
    /// - Unknown id is silently ignored. Callers are container methods
    ///   running under their own shard lock; a vanished registry entry
    ///   means a concurrent shutdown is racing the mutator, and the
    ///   entry's eventual `release` will fix `total_bytes` from the
    ///   stored `Entry::mem_bytes`.
    /// - Positive deltas do NOT re-check `max_bytes`. Cap enforcement on
    ///   container growth is the per-instance type's job (`Map::max_entries`,
    ///   `Channel` capacity, `Pool` size) — the global cap acts only at
    ///   `insert` time. Treating it otherwise would force every Map::set
    ///   to be fallible on a cap-overshoot it cannot easily roll back.
    /// - Negative deltas saturate at zero, so a stale undercount can't
    ///   wrap `Entry::mem_bytes` or `total_bytes`.
    pub fn adjust_mem_bytes(&self, id: SharedId, delta: isize) {
        if delta == 0 {
            return;
        }
        let Some(e) = self.entries.get(&id) else {
            return;
        };
        if delta > 0 {
            let d = delta as usize;
            e.mem_bytes.fetch_add(d, Ordering::Relaxed);
            self.total_bytes.fetch_add(d as u64, Ordering::Relaxed);
        } else {
            let d = delta.unsigned_abs();
            // saturating_sub + fetch_update on AtomicUsize: clamp at 0.
            let _ = e
                .mem_bytes
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                    Some(cur.saturating_sub(d))
                });
            let _ = self
                .total_bytes
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                    Some(cur.saturating_sub(d as u64))
                });
        }
    }

    /// Retain: atomically increment the entry's ext_refcount. Returns
    /// the new count, or -1 if the entry does not exist or its
    /// refcount already reached zero (a concurrent `release` has
    /// committed to eviction).
    ///
    /// Plain `fetch_add` is unsafe here: between a concurrent
    /// `release`'s `fetch_sub` returning 1 and its `entries.remove`,
    /// the entry is still in the map but already destined to die.
    /// A naive retain in that window would resurrect it (bump 0 → 1)
    /// and leave a phantom external reference no future release can
    /// drain. CAS-loop refuses to bump from zero.
    pub fn retain(&self, id: SharedId) -> i32 {
        let Some(e) = self.entries.get(&id) else {
            return -1;
        };
        let mut cur = e.ext_refcount.load(Ordering::Acquire);
        loop {
            if cur == 0 {
                return -1;
            }
            // AcqRel on success: `ext_refcount` is the entry's lifecycle
            // guard, not a metric. The Acquire half pairs with the Release
            // in `release`'s `fetch_sub` so a retained entry observes every
            // prior holder's writes; the Release half publishes this retain
            // to the thread that will eventually evict. Acquire on failure
            // re-reads a coherent `cur` for the next CAS attempt.
            match e.ext_refcount.compare_exchange_weak(
                cur,
                cur + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return (cur + 1) as i32,
                Err(actual) => cur = actual,
            }
        }
    }

    /// Release: atomically decrement ext_refcount. If it reaches 0 the
    /// entry is evicted from the registry (on_drop fires, totals adjust).
    /// Returns the new count, or -1 if the entry does not exist.
    pub fn release(&self, id: SharedId) -> i32 {
        // AcqRel: the Release half publishes this thread's writes to whoever
        // drops the last ref; the Acquire half ensures the thread that sees
        // `prev == 1` observes every other holder's writes before it runs
        // `on_drop` and evicts. Stronger than Arc's Release + Acquire-fence
        // pattern (the fence is folded into the RMW), kept uniform with
        // `retain` so the refcount has one consistent ordering story.
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
impl SharedRegistry {
    /// Build a standalone registry that is **not** registered with the
    /// global `OnceLock`. Use this in unit tests that need to exercise
    /// behaviour parametrised on `SharedConfig` — the global registry
    /// can only be initialised once per process.
    pub(crate) fn new_for_test(config: SharedConfig) -> Self {
        SharedRegistry {
            entries: DashMap::with_capacity(16),
            next_id: AtomicI64::new(1),
            total_bytes: AtomicU64::new(0),
            total_entries: AtomicU64::new(0),
            config,
            shutting_down: AtomicBool::new(false),
        }
    }
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
        fn as_any(&self) -> &dyn std::any::Any {
            self
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
    #[should_panic(expected = "type_tag mismatch")]
    fn insert_rejects_type_tag_mismatch() {
        // TestInner::type_tag() reports Counter; inserting it under a
        // different tag must trip the debug_assert in `insert`.
        let reg = fresh_registry();
        let _ = reg.insert(SharedType::Atomic, Arc::new(TestInner { bytes: 8 }));
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

    #[test]
    fn record_op_respects_metrics_enabled_flag() {
        use crate::plugins::ox_shared::config::LockDiagnosticsLevel;
        use crate::plugins::ox_shared::types::atomic::AtomicInner;
        use std::sync::atomic::Ordering;

        let make_config = |metrics_enabled: bool| SharedConfig {
            enabled: true,
            max_entries: 100,
            max_bytes: 1024,
            soft_limit_ratio: 0.7,
            metrics_enabled,
            introspection_enabled: true,
            introspection_preview_enabled: true,
            cycle_detect_depth: 16,
            cycle_detect_edges: 10_000,
            shutdown_timeout_seconds: 5.0,
            poison_strict: false,
            lock_diagnostics: LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
            preview_string_limit: 256,
            preview_array_limit: 20,
        };

        // metrics_enabled = false ⇒ ops stays at 0.
        let reg = SharedRegistry::new_for_test(make_config(false));
        let id = reg
            .insert(SharedType::Atomic, Arc::new(AtomicInner::new(0)))
            .expect("insert succeeds");
        let entry = reg.lookup(id).expect("entry exists");
        reg.record_op(&entry);
        assert_eq!(entry.ops.load(Ordering::Relaxed), 0);

        // metrics_enabled = true ⇒ ops increments.
        let reg = SharedRegistry::new_for_test(make_config(true));
        let id = reg
            .insert(SharedType::Atomic, Arc::new(AtomicInner::new(0)))
            .expect("insert succeeds");
        let entry = reg.lookup(id).expect("entry exists");
        reg.record_op(&entry);
        assert_eq!(entry.ops.load(Ordering::Relaxed), 1);
    }

    /// Pins the invariant that `retain` MUST NOT raise `ext_refcount`
    /// from 0 to 1. Hitting 0 means a concurrent `release` already
    /// committed to eviction; a successful retain there would
    /// resurrect an entry that the registry is about to drop and
    /// leave a phantom external reference no future `release` can
    /// balance (entries.get later returns None).
    ///
    /// The race is timing-dependent, so this is a stress test:
    /// thousands of insert/release-vs-retain races, asserting zero
    /// resurrections at the end.
    #[test]
    fn retain_does_not_resurrect_after_release_to_zero() {
        use crate::plugins::ox_shared::config::LockDiagnosticsLevel;
        use std::sync::atomic::AtomicU64 as StdAtomicU64;
        use std::sync::Arc as StdArc;
        use std::sync::Barrier;

        // Standalone registry with generous caps so 5k iterations
        // don't trip the default 100-entry limit when retain wins
        // the race (entry leaks until both threads drain).
        let reg = StdArc::new(SharedRegistry::new_for_test(SharedConfig {
            enabled: true,
            max_entries: 1_000_000,
            max_bytes: 1 << 30,
            soft_limit_ratio: 0.7,
            metrics_enabled: false,
            introspection_enabled: false,
            introspection_preview_enabled: false,
            cycle_detect_depth: 16,
            cycle_detect_edges: 10_000,
            shutdown_timeout_seconds: 5.0,
            poison_strict: false,
            lock_diagnostics: LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
            preview_string_limit: 256,
            preview_array_limit: 20,
        }));
        let resurrections = StdArc::new(StdAtomicU64::new(0));
        let iters = 5_000;

        for _ in 0..iters {
            let id = reg
                .insert(SharedType::Counter, Arc::new(TestInner { bytes: 8 }))
                .unwrap();

            let barrier = StdArc::new(Barrier::new(2));
            let reg_r = StdArc::clone(&reg);
            let reg_a = StdArc::clone(&reg);
            let b1 = StdArc::clone(&barrier);
            let b2 = StdArc::clone(&barrier);
            let resurrections_t = StdArc::clone(&resurrections);

            let t_release = std::thread::spawn(move || {
                b1.wait();
                reg_r.release(id);
            });
            let t_retain = std::thread::spawn(move || {
                b2.wait();
                let rc = reg_a.retain(id);
                // rc == 1 means retain just bumped ext_refcount from 0 to 1
                // — i.e. resurrected an entry release() was committing to
                // evict.
                if rc == 1 {
                    resurrections_t.fetch_add(1, Ordering::Relaxed);
                }
                // Balance a successful retain so total_entries drains
                // and the next iteration doesn't fight capacity caps.
                if rc > 0 {
                    reg_a.release(id);
                }
            });

            t_release.join().unwrap();
            t_retain.join().unwrap();
        }

        assert_eq!(
            resurrections.load(Ordering::Relaxed),
            0,
            "retain resurrected an entry that release() drove to ext_refcount=0"
        );
    }

    fn capped_config(max_entries: usize, max_bytes: u64) -> SharedConfig {
        use crate::plugins::ox_shared::config::LockDiagnosticsLevel;
        SharedConfig {
            enabled: true,
            max_entries,
            max_bytes,
            soft_limit_ratio: 0.7,
            metrics_enabled: false,
            introspection_enabled: false,
            introspection_preview_enabled: false,
            cycle_detect_depth: 16,
            cycle_detect_edges: 10_000,
            shutdown_timeout_seconds: 5.0,
            poison_strict: false,
            lock_diagnostics: LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
            preview_string_limit: 256,
            preview_array_limit: 20,
        }
    }

    /// Pins the cap invariant under concurrent insert: a load+check+
    /// fetch_add pattern lets N threads all read the same pre-cap
    /// value, all pass the `>` check, and all commit — overshooting
    /// `max_entries` by up to N. The fix reserves the slot with
    /// `fetch_add` first and rolls back on cap-miss; the post-condition
    /// is `total_entries == max_entries` no matter the thread count.
    #[test]
    fn concurrent_insert_respects_max_entries() {
        use std::sync::Arc as StdArc;
        use std::sync::Barrier;

        const THREADS: usize = 100;
        const CAP: usize = 50;

        let reg = StdArc::new(SharedRegistry::new_for_test(capped_config(CAP, u64::MAX)));
        let barrier = StdArc::new(Barrier::new(THREADS));
        let mut handles = Vec::with_capacity(THREADS);

        for _ in 0..THREADS {
            let reg = StdArc::clone(&reg);
            let barrier = StdArc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                reg.insert(SharedType::Counter, Arc::new(TestInner { bytes: 1 }))
            }));
        }

        let mut oks = 0usize;
        let mut errs = 0usize;
        for h in handles {
            match h.join().unwrap() {
                Ok(_) => oks += 1,
                Err(SharedError::CapacityExceeded) => errs += 1,
                Err(other) => panic!("unexpected error: {other:?}"),
            }
        }

        assert_eq!(oks, CAP, "exactly CAP inserts must succeed");
        assert_eq!(errs, THREADS - CAP, "the rest must hit CapacityExceeded");
        assert_eq!(
            reg.total_entries() as usize,
            CAP,
            "total_entries must not overshoot the cap after rollbacks"
        );
    }

    /// Same shape, but constrains `max_bytes` instead of `max_entries`.
    /// Each insert reports a fixed `mem_bytes`, so the byte-cap admits
    /// exactly `max_bytes / (mem + ENTRY_FIXED_OVERHEAD)` entries
    /// regardless of contention.
    #[test]
    fn concurrent_insert_respects_max_bytes() {
        use std::sync::Arc as StdArc;
        use std::sync::Barrier;

        const THREADS: usize = 100;
        const MEM_PER: u64 = 32;
        const PER_ENTRY: u64 = MEM_PER + ENTRY_FIXED_OVERHEAD as u64;
        const CAP_BYTES: u64 = PER_ENTRY * 50;

        let reg = StdArc::new(SharedRegistry::new_for_test(capped_config(
            10_000, CAP_BYTES,
        )));
        let barrier = StdArc::new(Barrier::new(THREADS));
        let mut handles = Vec::with_capacity(THREADS);

        for _ in 0..THREADS {
            let reg = StdArc::clone(&reg);
            let barrier = StdArc::clone(&barrier);
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                reg.insert(
                    SharedType::Counter,
                    Arc::new(TestInner {
                        bytes: MEM_PER as usize,
                    }),
                )
            }));
        }

        let mut oks = 0usize;
        for h in handles {
            if h.join().unwrap().is_ok() {
                oks += 1;
            }
        }

        let expected_oks = (CAP_BYTES / PER_ENTRY) as usize;
        assert_eq!(
            oks, expected_oks,
            "Ok count must equal max_bytes / (mem + overhead)"
        );
        assert!(
            reg.total_bytes() <= CAP_BYTES,
            "total_bytes ({}) must not overshoot CAP_BYTES ({})",
            reg.total_bytes(),
            CAP_BYTES
        );
        assert_eq!(
            reg.total_entries() as usize,
            expected_oks,
            "total_entries must match successful inserts (bytes-cap rollback also drops entries)"
        );
    }

    /// Pins the contract that `insert` books `ENTRY_FIXED_OVERHEAD` in
    /// addition to `inner.mem_bytes()`. Without this, an operator who
    /// sets `OX_SHARED_MAX_BYTES=128MiB` would see real RSS climb past a
    /// gigabyte for scalars whose inner content reports ~8 B but whose
    /// storage chain (Arc<Entry>, DashMap bucket, malloc prologues)
    /// dominates the actual footprint.
    #[test]
    fn insert_books_fixed_overhead() {
        let reg = SharedRegistry::new_for_test(capped_config(10, 1 << 20));
        assert_eq!(reg.total_bytes(), 0);
        reg.insert(SharedType::Counter, Arc::new(TestInner { bytes: 8 }))
            .unwrap();
        assert_eq!(
            reg.total_bytes(),
            (8 + ENTRY_FIXED_OVERHEAD) as u64,
            "total_bytes must include ENTRY_FIXED_OVERHEAD per insert"
        );
    }

    /// Pins the contract that `max_bytes` enforcement now includes
    /// `ENTRY_FIXED_OVERHEAD`. Without this test, a future change that
    /// silently removes the overhead from the cap-check path would
    /// only surface under concurrent stress
    /// (`concurrent_insert_respects_max_bytes`) — easier to misread as
    /// "flaky scheduling".
    ///
    /// Scenario: zero-content entries (`bytes: 0`) inserted serially.
    /// Each booked byte comes from `ENTRY_FIXED_OVERHEAD` alone. The
    /// cap admits exactly `floor(max_bytes / ENTRY_FIXED_OVERHEAD)`
    /// entries; the next insert must fail `CapacityExceeded`.
    #[test]
    fn max_bytes_cap_counts_overhead_for_zero_content_entries() {
        const N: u64 = 5;
        let cap = N * ENTRY_FIXED_OVERHEAD as u64;
        let reg = SharedRegistry::new_for_test(capped_config(1_000, cap));

        for i in 0..N {
            reg.insert(SharedType::Counter, Arc::new(TestInner { bytes: 0 }))
                .unwrap_or_else(|e| panic!("insert {i}/{N} must succeed: {e:?}"));
        }
        assert_eq!(reg.total_bytes(), cap);
        assert_eq!(reg.total_entries(), N);

        let err = reg
            .insert(SharedType::Counter, Arc::new(TestInner { bytes: 0 }))
            .expect_err("(N+1)th insert must trip max_bytes");
        assert_eq!(err, SharedError::CapacityExceeded);
        assert_eq!(
            reg.total_bytes(),
            cap,
            "rolled-back insert must leave total_bytes untouched"
        );
        assert_eq!(reg.total_entries(), N);
    }

    /// Pins the contract that `adjust_mem_bytes` keeps `total_bytes`
    /// and `Entry::mem_bytes` in sync. Adds, removes, and a negative
    /// delta that would underflow saturate at zero.
    #[test]
    fn adjust_mem_bytes_tracks_growth_and_shrink() {
        let reg = SharedRegistry::new_for_test(capped_config(10, 1 << 20));
        let id = reg
            .insert(SharedType::Counter, Arc::new(TestInner { bytes: 0 }))
            .unwrap();
        let baseline = reg.total_bytes();

        reg.adjust_mem_bytes(id, 320);
        assert_eq!(reg.total_bytes(), baseline + 320);

        reg.adjust_mem_bytes(id, -100);
        assert_eq!(reg.total_bytes(), baseline + 220);

        // Underflow guard: subtracting more than current must clamp,
        // not wrap.
        reg.adjust_mem_bytes(id, -(i64::from(u32::MAX) as isize));
        assert_eq!(reg.total_bytes(), 0);
    }
}
