//! SharedRegistry — process-global entry store. Arc-refcount lifecycle.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};
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
/// 200 is a consciously conservative pick inside the 184–216 B
/// structural range — closer to the upper edge than the middle so
/// that allocator quirks (bucket-size rounding under glibc/musl,
/// per-thread arena headers, mimalloc segment metadata amortised
/// across many entries) stay accounted for under the cap rather than
/// leaking out over it. **The number is a static accounting estimate, NOT
/// measured against a heap profiler** — calibrating against heaptrack
/// / `jemalloc_stats_print` / `mi_stats_print` on a target platform is
/// a nice-to-have follow-up that would tighten the bound, but adds CI
/// dependencies. For now the doc and the code are honest about being
/// a conservative estimate inside a known range.
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

/// Sentinel written into `Entry::magic` at construction and overwritten
/// with `0xDEAD_BEEF` in `Entry::drop`. FFI entrypoints `debug_assert`
/// against it to turn a use-after-free (handle dereferenced after the
/// backing `Arc` was dropped) into an explicit panic in debug builds.
/// Zero-cost in release.
pub const ENTRY_MAGIC: u32 = 0x0570_5048; // arbitrary fixed marker

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

/// A registry entry. Its lifetime is governed by a single mechanism:
/// the `Arc<Entry>` strong count. The PHP wrapper's `SharedHandle`
/// holds one strong reference via `Arc::into_raw`; `retain`/`release`
/// are now `Arc` clone/drop. The registry's `entries` map holds only a
/// `Weak<Entry>`, so it never anchors the entry. When the last strong
/// `Arc` drops, `Entry::drop` self-deregisters from the registry.
pub struct Entry {
    /// `ENTRY_MAGIC` while alive; `0xDEAD_BEEF` after `Drop` has run.
    pub magic: u32,
    pub id: SharedId,
    pub type_tag: SharedType,
    pub inner: Arc<dyn SharedInner>,
    pub created_at: Instant,
    pub ops: AtomicU64,
    pub mem_bytes: AtomicUsize,
    /// Back-reference to the owning registry, set in `insert`. Used by
    /// `Drop` to self-deregister. `&'static` because the production
    /// registry lives in a `OnceLock` static; `new_for_test` leaks its
    /// registry via `Box::leak` so test entries also get a `'static`
    /// reference. An `Entry` must never outlive its registry.
    pub(crate) registry: &'static SharedRegistry,
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("magic", &format_args!("{:#010x}", self.magic))
            .field("id", &self.id)
            .field("type_tag", &self.type_tag)
            .field("ops", &self.ops.load(Ordering::Relaxed))
            .field("mem_bytes", &self.mem_bytes.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl Drop for Entry {
    /// Runs when the last `Arc<Entry>` is dropped. Self-deregisters
    /// from the owning registry: poisons `magic`, fires
    /// `inner.on_drop()`, removes the `Weak` index entry, and adjusts
    /// the registry totals. This is the single point where an entry
    /// leaves the registry — there is no longer an `ext_refcount` /
    /// `release` path.
    fn drop(&mut self) {
        self.magic = 0xDEAD_BEEF;
        self.inner.on_drop();
        self.registry.entries.remove(&self.id);
        self.registry.total_entries.fetch_sub(1, Ordering::Relaxed);
        self.registry.total_bytes.fetch_sub(
            self.mem_bytes.load(Ordering::Relaxed) as u64,
            Ordering::Relaxed,
        );
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
    /// Pure id index — holds `Weak`, never keeps an `Entry` alive. The
    /// `Arc<Entry>` lifetime anchor lives in the PHP wrapper's handle.
    /// Used only by tag-7 cross-thread transfer (`lookup`) and shutdown
    /// enumeration (`drain`, `iter_entries`). NOT touched on the FFI
    /// hot path.
    entries: DashMap<SharedId, Weak<Entry>>,
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

    /// Insert a new entry. Returns the strong `Arc<Entry>` — the caller
    /// owns the entry's lifetime and moves it into the PHP handle via
    /// `Arc::into_raw`. Fails with CapacityExceeded if hard caps would
    /// be breached.
    ///
    /// Reserves capacity via `fetch_add` and rolls back on cap-miss
    /// (Tokio-style semaphore pattern). A concurrent burst may briefly
    /// observe `total_*` above the configured cap (for the duration of
    /// the failing thread's rollback — sub-µs); the post-condition
    /// `total_* <= max_*` is preserved.
    ///
    /// Takes `&'static self` so the created `Entry` can store a
    /// `&'static` back-reference to this registry for `Drop`-time
    /// self-deregistration.
    pub fn insert(
        &'static self,
        type_tag: SharedType,
        inner: Arc<dyn SharedInner>,
    ) -> Result<Arc<Entry>, SharedError> {
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
            magic: ENTRY_MAGIC,
            id,
            type_tag,
            inner,
            created_at: Instant::now(),
            ops: AtomicU64::new(0),
            mem_bytes: AtomicUsize::new(mem),
            registry: self,
        });
        // The map holds only a Weak — it never anchors the Entry. The
        // returned Arc is the sole strong ref; the caller moves it into
        // the PHP handle via Arc::into_raw.
        self.entries.insert(id, Arc::downgrade(&entry));
        Ok(entry)
    }

    /// Resolve an id to a strong `Arc<Entry>`. Used **only** on the
    /// tag-7 cross-thread deserialize path and in tests — never on the
    /// in-process FFI hot path, which dereferences the handle pointer
    /// directly. Returns `StaleHandle` if the id is unknown or the
    /// entry's last `Arc` has already dropped (a dead `Weak`).
    pub fn lookup(&self, id: SharedId) -> Result<Arc<Entry>, SharedError> {
        self.entries
            .get(&id)
            .and_then(|w| w.upgrade())
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
        let Some(e) = self.entries.get(&id).and_then(|w| w.upgrade()) else {
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

    /// Enumerate every live entry. Dead `Weak`s (an `Entry` mid-drop)
    /// are filtered out via `upgrade()`. Consumers: introspection /
    /// observability.
    pub fn iter_entries(&self) -> impl Iterator<Item = Arc<Entry>> + '_ {
        self.entries.iter().filter_map(|w| w.value().upgrade())
    }

    /// Drain: wake blocked ops, mark shutting-down.
    /// No-op for atomic types because they don't block.
    pub fn drain(&self) {
        self.shutting_down.store(true, Ordering::Release);
        for w in self.entries.iter() {
            if let Some(entry) = w.value().upgrade() {
                entry.inner.on_shutdown_notify();
            }
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

/// Clone the handle's strong reference. Given a live `entry_ptr`
/// (`Arc::into_raw` pointer), bumps the strong count and returns the
/// same pointer — the caller now owns an additional strong ref and
/// must balance it with `oxphp_shared_handle_drop`. Returns NULL if
/// `entry_ptr` is NULL.
///
/// # Safety
/// `entry_ptr` must be NULL or a pointer obtained from `Arc::into_raw`
/// on an `Arc<Entry>` that is still alive.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_handle_clone(entry_ptr: *const Entry) -> *const Entry {
    if entry_ptr.is_null() {
        return std::ptr::null();
    }
    // SAFETY: caller guarantees entry_ptr is a live Arc::into_raw ptr.
    unsafe { Arc::increment_strong_count(entry_ptr) };
    entry_ptr
}

/// Drop one strong reference owned via `entry_ptr`. Reconstitutes the
/// `Arc` and drops it; when it is the last strong ref, `Entry::drop`
/// self-deregisters. No-op on NULL.
///
/// # Safety
/// `entry_ptr` must be NULL or a pointer obtained from `Arc::into_raw`
/// that has not already been passed to this function. Double-free is
/// undefined behaviour — see the contract note in `oxphp_bridge.h`.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_handle_drop(entry_ptr: *const Entry) {
    if entry_ptr.is_null() {
        return;
    }
    // SAFETY: caller guarantees a live, not-yet-dropped Arc::into_raw ptr.
    unsafe { drop(Arc::from_raw(entry_ptr)) };
}

/// Read the registry id of the entry behind `entry_ptr`. Used by the
/// tag-7 serializer to write the wire id. Returns 0 on NULL (0 is
/// never a valid id — `next_id` starts at 1).
///
/// # Safety
/// `entry_ptr` must be NULL or a live `Arc::into_raw` pointer.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_entry_id(entry_ptr: *const Entry) -> u64 {
    if entry_ptr.is_null() {
        return 0;
    }
    // SAFETY: caller guarantees a live Arc::into_raw ptr; the Entry is
    // alive because the caller holds a strong ref through it.
    let entry = unsafe { &*entry_ptr };
    debug_assert_eq!(
        entry.magic, ENTRY_MAGIC,
        "oxphp_shared_entry_id on freed Entry"
    );
    entry.id
}

/// Resolve a wire id (tag-7 deserialize) to a fresh strong reference.
/// Looks the id up in the `Weak` index and upgrades it; returns an
/// `Arc::into_raw` pointer the receiving handle owns, or NULL if the
/// entry died in the transfer window (a correct `StaleHandle`).
#[no_mangle]
pub extern "C" fn oxphp_shared_handle_from_id(id: u64) -> *const Entry {
    match REGISTRY.get() {
        Some(reg) => match reg.lookup(id) {
            Ok(arc) => Arc::into_raw(arc),
            Err(_) => std::ptr::null(),
        },
        None => std::ptr::null(),
    }
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
    /// global `OnceLock`, leaked to `'static` so `Entry::registry` can
    /// hold a `&'static` reference. Use in unit tests that need to
    /// exercise behaviour parametrised on `SharedConfig`. The leak is
    /// intentional and bounded — test processes are short-lived.
    pub(crate) fn new_for_test(config: SharedConfig) -> &'static SharedRegistry {
        Box::leak(Box::new(SharedRegistry {
            entries: DashMap::with_capacity(16),
            next_id: AtomicI64::new(1),
            total_bytes: AtomicU64::new(0),
            total_entries: AtomicU64::new(0),
            config,
            shutting_down: AtomicBool::new(false),
        }))
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
        let arc = reg
            .insert(SharedType::Counter, Arc::new(TestInner { bytes: 16 }))
            .unwrap();
        let id = arc.id;
        assert!(reg.lookup(id).is_ok());
        assert_eq!(arc.type_tag, SharedType::Counter);
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
        // Hold the Arc<Entry> for the assertion — without it, the entry
        // would self-deregister on drop and total_entries would not
        // advance.
        let _arc = reg
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
        let entry = reg
            .insert(SharedType::Atomic, Arc::new(AtomicInner::new(0)))
            .expect("insert succeeds");
        reg.record_op(&entry);
        assert_eq!(entry.ops.load(Ordering::Relaxed), 0);

        // metrics_enabled = true ⇒ ops increments.
        let reg = SharedRegistry::new_for_test(make_config(true));
        let entry = reg
            .insert(SharedType::Atomic, Arc::new(AtomicInner::new(0)))
            .expect("insert succeeds");
        reg.record_op(&entry);
        assert_eq!(entry.ops.load(Ordering::Relaxed), 1);
    }

    // The previous `retain_does_not_resurrect_after_release_to_zero`
    // stress test is deleted: with `Arc<Entry>` as the sole lifetime
    // mechanism, retain/release became `Arc::clone` / `drop` and the
    // resurrection race no longer exists as a class.

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

        // Collect the returned `Arc<Entry>`s into a Vec to keep their
        // strong refs alive until the `total_entries` assertion below —
        // dropping an `Arc<Entry>` triggers self-deregistration.
        let mut held: Vec<Arc<Entry>> = Vec::new();
        let mut errs = 0usize;
        for h in handles {
            match h.join().unwrap() {
                Ok(arc) => held.push(arc),
                Err(SharedError::CapacityExceeded) => errs += 1,
                Err(other) => panic!("unexpected error: {other:?}"),
            }
        }
        let oks = held.len();

        assert_eq!(oks, CAP, "exactly CAP inserts must succeed");
        assert_eq!(errs, THREADS - CAP, "the rest must hit CapacityExceeded");
        assert_eq!(
            reg.total_entries() as usize,
            CAP,
            "total_entries must not overshoot the cap after rollbacks"
        );
        drop(held);
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

        let mut held: Vec<Arc<Entry>> = Vec::new();
        for h in handles {
            if let Ok(arc) = h.join().unwrap() {
                held.push(arc);
            }
        }
        let oks = held.len();

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
        drop(held);
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
        // Hold the Arc — otherwise the entry self-deregisters and
        // total_bytes returns to 0 before the assert.
        let _arc = reg
            .insert(SharedType::Counter, Arc::new(TestInner { bytes: 8 }))
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

        // Collect the Arcs so the entries stay alive for the cap check.
        let mut held: Vec<Arc<Entry>> = Vec::new();
        for i in 0..N {
            held.push(
                reg.insert(SharedType::Counter, Arc::new(TestInner { bytes: 0 }))
                    .unwrap_or_else(|e| panic!("insert {i}/{N} must succeed: {e:?}")),
            );
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
        drop(held);
    }

    /// Pins the contract that `adjust_mem_bytes` keeps `total_bytes`
    /// and `Entry::mem_bytes` in sync. Adds, removes, and a negative
    /// delta that would underflow saturate at zero.
    #[test]
    fn adjust_mem_bytes_tracks_growth_and_shrink() {
        let reg = SharedRegistry::new_for_test(capped_config(10, 1 << 20));
        let arc = reg
            .insert(SharedType::Counter, Arc::new(TestInner { bytes: 0 }))
            .unwrap();
        let id = arc.id;
        let baseline = reg.total_bytes();

        reg.adjust_mem_bytes(id, 320);
        assert_eq!(reg.total_bytes(), baseline + 320);

        reg.adjust_mem_bytes(id, -100);
        assert_eq!(reg.total_bytes(), baseline + 220);

        // Underflow guard: subtracting more than current must clamp,
        // not wrap.
        reg.adjust_mem_bytes(id, -(i64::from(u32::MAX) as isize));
        assert_eq!(reg.total_bytes(), 0);
        drop(arc);
    }

    /// `*const Entry` is not `Send` by default. The stress tests below
    /// move raw entry pointers into worker threads via this newtype —
    /// the underlying `Arc::into_raw` pointer is `Send` because
    /// `Entry: Send + Sync` and the strong-ref it represents is
    /// independent of any one thread.
    #[derive(Copy, Clone)]
    struct EntryPtr(*const Entry);
    unsafe impl Send for EntryPtr {}

    impl EntryPtr {
        /// Consume the wrapper and yield the inner pointer. Used inside
        /// `move ||` thread closures: Rust 2021 disjoint capture would
        /// otherwise project `self.0` and capture only the raw pointer
        /// (not Send) rather than the whole `EntryPtr` (Send).
        fn raw(self) -> *const Entry {
            self.0
        }
    }

    /// Test inner whose `on_drop` bumps a shared counter, so a test
    /// can observe exactly when the `Entry` is freed.
    struct DropCountInner {
        on_drop_calls: &'static AtomicU64,
    }

    impl SharedInner for DropCountInner {
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
            0
        }
        fn on_drop(&self) {
            self.on_drop_calls.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Many threads clone and drop strong refs to one entry via the
    /// `oxphp_shared_handle_clone` / `oxphp_shared_handle_drop` FFI.
    /// The entry must be freed exactly once, and only after the last
    /// strong ref drops — never while a clone is outstanding.
    #[test]
    fn handle_clone_drop_balance() {
        use std::sync::Barrier;

        let on_drop_calls: &'static AtomicU64 = Box::leak(Box::new(AtomicU64::new(0)));
        let reg = SharedRegistry::new_for_test(capped_config(10, 1 << 20));
        let entry = reg
            .insert(
                SharedType::Counter,
                Arc::new(DropCountInner { on_drop_calls }),
            )
            .unwrap();
        let id = entry.id;
        let base_ptr = EntryPtr(Arc::into_raw(entry)); // creating-wrapper strong ref

        const THREADS: usize = 16;
        const ITERS: usize = 1_000;
        let barrier = std::sync::Arc::new(Barrier::new(THREADS));
        let mut handles = Vec::new();
        for _ in 0..THREADS {
            let barrier = std::sync::Arc::clone(&barrier);
            let p = base_ptr;
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                let raw = p.raw();
                for _ in 0..ITERS {
                    // SAFETY: raw is a live Arc::into_raw ptr held by
                    // the main thread for the whole test.
                    let cloned = unsafe { oxphp_shared_handle_clone(raw) };
                    assert!(!cloned.is_null());
                    unsafe { oxphp_shared_handle_drop(cloned) };
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }

        // All clones balanced; the entry is still alive (base_ptr ref).
        assert_eq!(on_drop_calls.load(Ordering::Relaxed), 0);
        assert!(reg.lookup(id).is_ok());

        // Drop the last strong ref — entry frees now, exactly once.
        unsafe { oxphp_shared_handle_drop(base_ptr.raw()) };
        assert_eq!(on_drop_calls.load(Ordering::Relaxed), 1);
    }

    /// Races `oxphp_shared_handle_from_id` (receiver side of a tag-7
    /// transfer) against the drop of the source entry's last strong
    /// ref. Every outcome must be either a valid strong pointer or a
    /// clean NULL — never a use-after-free.
    ///
    /// Uses `fresh_registry()` (the global path), not `new_for_test`,
    /// because `oxphp_shared_handle_from_id` reads the global REGISTRY
    /// static.
    #[test]
    fn tag7_transfer_race_is_clean() {
        use std::sync::Barrier;

        let reg = fresh_registry();
        const ITERS: usize = 5_000;

        for _ in 0..ITERS {
            let entry = reg
                .insert(SharedType::Counter, Arc::new(TestInner { bytes: 8 }))
                .unwrap();
            let id = entry.id;
            let src_ptr = EntryPtr(Arc::into_raw(entry));

            let barrier = std::sync::Arc::new(Barrier::new(2));
            let b1 = std::sync::Arc::clone(&barrier);
            let b2 = std::sync::Arc::clone(&barrier);

            let t_drop = std::thread::spawn(move || {
                let raw = src_ptr.raw();
                b1.wait();
                // SAFETY: raw is a live Arc::into_raw ptr, dropped once.
                unsafe { oxphp_shared_handle_drop(raw) };
            });
            let t_recv = std::thread::spawn(move || {
                b2.wait();
                let got = oxphp_shared_handle_from_id(id);
                if !got.is_null() {
                    // Got a strong ref — it must be a valid Entry.
                    // SAFETY: handle_from_id returned a live Arc::into_raw ptr.
                    let e = unsafe { &*got };
                    assert_eq!(e.magic, ENTRY_MAGIC, "handle_from_id returned freed Entry");
                    assert_eq!(e.id, id);
                    unsafe { oxphp_shared_handle_drop(got) };
                }
            });

            t_drop.join().unwrap();
            t_recv.join().unwrap();
        }
    }

    /// After the last strong ref to an entry drops, `Entry::drop` must
    /// have removed the `Weak` index entry and decremented both totals
    /// by exactly that entry's contribution.
    #[test]
    fn entry_drop_self_deregisters() {
        let reg = SharedRegistry::new_for_test(capped_config(10, 1 << 20));
        let entry = reg
            .insert(SharedType::Counter, Arc::new(TestInner { bytes: 64 }))
            .unwrap();
        let id = entry.id;
        let booked = 64 + ENTRY_FIXED_OVERHEAD as u64;

        assert_eq!(reg.total_entries(), 1);
        assert_eq!(reg.total_bytes(), booked);
        assert!(reg.lookup(id).is_ok());

        drop(entry); // last strong ref → Entry::drop

        assert!(
            reg.lookup(id).is_err(),
            "Weak index entry must be gone after Entry::drop"
        );
        assert_eq!(reg.total_entries(), 0, "total_entries decremented");
        assert_eq!(reg.total_bytes(), 0, "total_bytes decremented");
    }
}
