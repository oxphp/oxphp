//! `Shared\Map` — concurrent `Arc<str> → SharedValue` store.
//!
//! Built on DashMap with nested-Shareable refcount management: `set`
//! atomically swaps (retain new, caller inherits the displaced retain);
//! `clear` and `on_drop` release the Map's hold on every nested
//! `SharedValue::Shared`. `remove` hands the prev value to the caller
//! without releasing — the FFI layer uses that inheritance window to
//! build a PHP wrapper (+retain) before calling [`sv_release_nested`]
//! to drop the Map's hold.
//!
//! Additional layers: cycle detection, per-instance cap, atomic RMW
//! (setIfAbsent / update / getOrSet), FFI + PHP class, and batched
//! ops.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

use dashmap::DashMap;

use crate::plugins::ox_shared::cycle::{format_cycle_path, would_create_cycle, CycleError};
use crate::plugins::ox_shared::error::{set_last_error, SharedError};
use crate::plugins::ox_shared::registry::{SharedId, SharedInner, SharedType, REGISTRY};
use crate::plugins::ox_shared::value::{
    collect_shared_refs, sv_release_nested, sv_retain_nested, SharedRef, SharedValue,
};

/// Rust-side storage for one `Shared\Map` instance.
pub struct MapInner {
    entries: DashMap<Arc<str>, SharedValue>,
    /// Per-instance cap; `None` = unbounded (subject only to the global
    /// `SHARED_MAX_ENTRIES`).
    max_entries: Option<usize>,
    /// Strict entry-count mirror of `entries.len()`. Maintained via
    /// atomic CAS in `set` (for cap enforcement) and atomic decrement
    /// in `remove` / reset in `clear`. DashMap's `.len()` aggregates
    /// shard counts with no synchronisation guarantee wrt cap checks,
    /// so we keep this parallel counter.
    count: AtomicUsize,
    /// The Map's own registry id, bound once by the creating FFI path
    /// via [`MapInner::bind_id`]. `None` before bind (or in Rust-only
    /// tests that skip registry insertion) — cycle detection is then
    /// a no-op because nothing in the registry can reach this Map.
    self_id: OnceLock<SharedId>,
}

impl MapInner {
    pub fn new(max_entries: Option<usize>) -> Self {
        Self {
            entries: DashMap::new(),
            max_entries,
            count: AtomicUsize::new(0),
            self_id: OnceLock::new(),
        }
    }

    pub fn max_entries(&self) -> Option<usize> {
        self.max_entries
    }

    /// Bind this Map to its registry id. Called exactly once by the
    /// creating FFI path right after `registry.insert`. Subsequent
    /// calls are silently ignored (OnceLock semantics).
    pub fn bind_id(&self, id: SharedId) {
        let _ = self.self_id.set(id);
    }

    pub fn self_id(&self) -> Option<SharedId> {
        self.self_id.get().copied()
    }

    /// Insert or replace. Returns the previous value, if any.
    ///
    /// Runs the cycle check BEFORE any mutation so a rejected insert
    /// leaves the Map untouched and no retain counts leak. The Map's
    /// own id must have been bound via [`bind_id`] for the check to
    /// fire — otherwise nothing in the registry can reach this Map
    /// and cycles are impossible.
    ///
    /// Refcount contract: any `SharedValue::Shared` reachable in `value`
    /// is retained (Map gains hold) on success. The returned prev value
    /// (if any) carries the Map's old retain — **caller MUST call
    /// [`sv_release_nested`] on the returned value once finished**
    /// (directly, or after building a PHP wrapper that assumes the
    /// retain). This asymmetry lets FFI reorder "wrapper retain first,
    /// then drop Map's hold" without risking mid-operation eviction.
    ///
    /// On cycle:
    /// - Last-error thread-local is populated with the reachable path
    ///   (e.g. `"cycle would form: #3 → #5 → #42"`).
    /// - `SharedError::Cycle` is returned; the FFI layer maps this to
    ///   `OxPHP\Shared\CycleException`.
    ///
    /// [`bind_id`]: MapInner::bind_id
    pub fn set(
        &self,
        key: Arc<str>,
        value: SharedValue,
    ) -> Result<Option<SharedValue>, SharedError> {
        let reg = REGISTRY.get();

        // Pre-mutation: cycle check. Scalar values skip the walk (zero-cost).
        if let Some(reg) = reg {
            self.check_cycles(reg, &value)?;
        }

        // Atomic insert path. The shard lock inside `entry(key)` serialises
        // all writes to this key; the cap enforcement uses a separate
        // CAS-loop on `self.count` to stay consistent across shards.
        let e = self.entries.entry(key);
        match e {
            dashmap::Entry::Occupied(mut occ) => {
                // Replace — count unchanged, no cap check.
                if let Some(reg) = reg {
                    sv_retain_nested(&value, reg);
                }
                let prev = std::mem::replace(occ.get_mut(), value);
                Ok(Some(prev))
            }
            dashmap::Entry::Vacant(vac) => {
                // New key — enforce per-instance cap (if set) atomically.
                if let Some(max) = self.max_entries {
                    if self
                        .count
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |c| {
                            if c < max {
                                Some(c + 1)
                            } else {
                                None
                            }
                        })
                        .is_err()
                    {
                        set_last_error(format!(
                            "Shared\\Map capacity exceeded: {max}/{max} entries; \
                             raise `new Shared\\Map(maxEntries: ...)` or remove \
                             keys first"
                        ));
                        return Err(SharedError::CapacityExceeded);
                    }
                } else {
                    self.count.fetch_add(1, Ordering::Relaxed);
                }

                if let Some(reg) = reg {
                    sv_retain_nested(&value, reg);
                }
                vac.insert(value);
                Ok(None)
            }
        }
    }

    /// Run the cycle walker for every `SharedValue::Shared` reachable
    /// from `value`. Returns early on first cycle (short-circuit).
    /// No-op if this Map has no `self_id` bound yet.
    fn check_cycles(
        &self,
        reg: &'static crate::plugins::ox_shared::registry::SharedRegistry,
        value: &SharedValue,
    ) -> Result<(), SharedError> {
        let Some(self_id) = self.self_id() else {
            return Ok(());
        };

        let mut roots = Vec::new();
        collect_shared_refs(value, &mut roots);
        if roots.is_empty() {
            return Ok(());
        }

        let cfg = reg.config();
        let depth = cfg.cycle_detect_depth;
        let edges = cfg.cycle_detect_edges;

        for root in roots {
            let children_of = |id: SharedId, out: &mut Vec<SharedRef>| {
                if let Ok(entry) = reg.lookup(id) {
                    entry.inner.children(out);
                }
            };
            match would_create_cycle(root, self_id, depth, edges, children_of) {
                Ok(()) => {}
                Err(CycleError::CycleFound(path)) => {
                    set_last_error(format!(
                        "cycle would form: {} (inserting into #{self_id})",
                        format_cycle_path(&path)
                    ));
                    return Err(SharedError::Cycle);
                }
                Err(CycleError::DepthExceeded) => {
                    set_last_error(format!(
                        "cycle detection depth limit ({depth}) exceeded; raise \
                         SHARED_CYCLE_DETECT_DEPTH or break the graph"
                    ));
                    return Err(SharedError::Cycle);
                }
                Err(CycleError::EdgeLimitExceeded) => {
                    set_last_error(format!(
                        "cycle detection edge limit ({edges}) exceeded; raise \
                         SHARED_CYCLE_DETECT_EDGES or break the graph"
                    ));
                    return Err(SharedError::Cycle);
                }
            }
        }
        Ok(())
    }

    pub fn get(&self, key: &str) -> Option<SharedValue> {
        self.entries.get(key).map(|r| r.value().clone())
    }

    /// Core single-key insert without the cycle check. Used by
    /// [`set_many`] after a single batched cycle check covers every
    /// value in the incoming batch — folding N per-key cycle walks
    /// into one is the main reason the batched API outperforms
    /// `N × set` at the PHP layer (spec §Performance target).
    ///
    /// Same retain/release contract as [`set`]: caller inherits the
    /// returned previous value's retain.
    fn set_without_cycle_check(
        &self,
        key: Arc<str>,
        value: SharedValue,
    ) -> Result<Option<SharedValue>, SharedError> {
        let reg = REGISTRY.get();
        let e = self.entries.entry(key);
        match e {
            dashmap::Entry::Occupied(mut occ) => {
                if let Some(reg) = reg {
                    sv_retain_nested(&value, reg);
                }
                let prev = std::mem::replace(occ.get_mut(), value);
                Ok(Some(prev))
            }
            dashmap::Entry::Vacant(vac) => {
                if let Some(max) = self.max_entries {
                    if self
                        .count
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |c| {
                            if c < max {
                                Some(c + 1)
                            } else {
                                None
                            }
                        })
                        .is_err()
                    {
                        set_last_error(format!(
                            "Shared\\Map capacity exceeded: {max}/{max} entries; \
                             raise `new Shared\\Map(maxEntries: ...)` or remove \
                             keys first"
                        ));
                        return Err(SharedError::CapacityExceeded);
                    }
                } else {
                    self.count.fetch_add(1, Ordering::Relaxed);
                }

                if let Some(reg) = reg {
                    sv_retain_nested(&value, reg);
                }
                vac.insert(value);
                Ok(None)
            }
        }
    }

    /// Batched insert. Runs the cycle walker **once** over every value
    /// in the batch, then replays the per-key insert loop without
    /// re-running cycle detection. This is the shape the PHP-level
    /// perf gate (spec §Performance target: `setMany(100) ≥ 3× 100×set`)
    /// actually needs — PHP-engine dispatch is already amortised by the
    /// single FFI crossing in the caller, and per-key cycle-walk
    /// allocations are the remaining hot spot.
    ///
    /// On first error (cycle, capacity, type): previously-committed
    /// writes in this batch are kept (partial-apply, matches set_many's
    /// documented per-key semantics); [`out_inserted`] in the FFI path
    /// reports how many entries landed before the bail. All already-
    /// displaced prev values are released before returning.
    pub fn set_many_batch(
        &self,
        batch: Vec<(Arc<str>, SharedValue)>,
    ) -> Result<usize, (SharedError, usize)> {
        let reg = REGISTRY.get();

        // 1. Single cycle check covering every Shared ref in the batch.
        //    Skipped when the Map has no self_id bound or the batch is
        //    free of Shareable values (common hot path: scalar maps).
        if let Some(reg) = reg {
            if let Some(self_id) = self.self_id() {
                let mut all_roots: Vec<SharedRef> = Vec::new();
                for (_, v) in &batch {
                    collect_shared_refs(v, &mut all_roots);
                }
                if !all_roots.is_empty() {
                    let cfg = reg.config();
                    let depth = cfg.cycle_detect_depth;
                    let edges = cfg.cycle_detect_edges;
                    for root in all_roots {
                        let children_of = |id: SharedId, out: &mut Vec<SharedRef>| {
                            if let Ok(entry) = reg.lookup(id) {
                                entry.inner.children(out);
                            }
                        };
                        match would_create_cycle(root, self_id, depth, edges, children_of) {
                            Ok(()) => {}
                            Err(CycleError::CycleFound(path)) => {
                                set_last_error(format!(
                                    "cycle would form: {} (inserting into #{self_id})",
                                    format_cycle_path(&path)
                                ));
                                return Err((SharedError::Cycle, 0));
                            }
                            Err(CycleError::DepthExceeded) => {
                                set_last_error(format!(
                                    "cycle detection depth limit ({depth}) exceeded; \
                                     raise SHARED_CYCLE_DETECT_DEPTH or break the graph"
                                ));
                                return Err((SharedError::Cycle, 0));
                            }
                            Err(CycleError::EdgeLimitExceeded) => {
                                set_last_error(format!(
                                    "cycle detection edge limit ({edges}) exceeded; \
                                     raise SHARED_CYCLE_DETECT_EDGES or break the graph"
                                ));
                                return Err((SharedError::Cycle, 0));
                            }
                        }
                    }
                }
            }
        }

        // 2. Unbounded fast path: skip the per-key `count` CAS. Reserve
        //    `batch.len()` slots up front with a single atomic add, then
        //    refund overwrites (where `entries.insert` returned `Some`)
        //    with one atomic subtract at the end. Shaves N - 2 atomic
        //    ops — the main Rust-side win after dropping the per-key
        //    cycle walk.
        if self.max_entries.is_none() {
            self.count.fetch_add(batch.len(), Ordering::Relaxed);
            let mut overwrites: usize = 0;
            let mut inserted = 0;
            for (k, v) in batch {
                if let Some(reg) = reg {
                    sv_retain_nested(&v, reg);
                }
                if let Some(prev) = self.entries.insert(k, v) {
                    overwrites += 1;
                    if let Some(reg) = reg {
                        sv_release_nested(&prev, reg);
                    }
                }
                inserted += 1;
            }
            if overwrites > 0 {
                self.count.fetch_sub(overwrites, Ordering::Relaxed);
            }
            return Ok(inserted);
        }

        // 3. Capped path: fall back to the per-key helper so cap
        //    enforcement is strict — a single optimistic reservation
        //    would risk over-reserving vs `max_entries` when the batch
        //    contains overwrites.
        let mut inserted = 0;
        for (k, v) in batch {
            match self.set_without_cycle_check(k, v) {
                Ok(prev) => {
                    if let Some(prev) = prev {
                        if let Some(reg) = reg {
                            sv_release_nested(&prev, reg);
                        }
                    }
                    inserted += 1;
                }
                Err(e) => return Err((e, inserted)),
            }
        }
        Ok(inserted)
    }

    /// Atomic insert-if-absent. Returns `true` when the key was missing
    /// and `value` was stored, `false` when the key already existed
    /// (and `value` was discarded). Cycle + cap checks run before the
    /// shard lock so `Vacant` never observes a half-done retain.
    ///
    /// PHP counterpart: `Shared\Map::setIfAbsent($key, $value): bool`.
    pub fn set_if_absent(&self, key: Arc<str>, value: SharedValue) -> Result<bool, SharedError> {
        let reg = REGISTRY.get();
        if let Some(reg) = reg {
            self.check_cycles(reg, &value)?;
        }

        let e = self.entries.entry(key);
        match e {
            dashmap::Entry::Occupied(_) => Ok(false),
            dashmap::Entry::Vacant(vac) => {
                if let Some(max) = self.max_entries {
                    if self
                        .count
                        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |c| {
                            if c < max {
                                Some(c + 1)
                            } else {
                                None
                            }
                        })
                        .is_err()
                    {
                        set_last_error(format!(
                            "Shared\\Map capacity exceeded: {max}/{max} entries; \
                             raise `new Shared\\Map(maxEntries: ...)` or remove \
                             keys first"
                        ));
                        return Err(SharedError::CapacityExceeded);
                    }
                } else {
                    self.count.fetch_add(1, Ordering::Relaxed);
                }
                if let Some(reg) = reg {
                    sv_retain_nested(&value, reg);
                }
                vac.insert(value);
                Ok(true)
            }
        }
    }

    /// Read-modify-write on `key`. `f` receives the current value
    /// (or `None` if the key is absent) and returns the new value to
    /// store — returning `None` removes the key.
    ///
    /// Returns the stored value (`Some`) or `None` if the key was removed
    /// or the closure returned `None` for an absent key.
    ///
    /// Atomicity note: the snapshot / compute / commit sequence is not
    /// CAS-linearised across concurrent writers on the same key — the
    /// commit goes through the regular [`set`] / [`remove`] path, so a
    /// racing write can interleave between the closure's observation
    /// and the store. Single-key atomicity is guaranteed *per phase*
    /// (shard-lock granularity), which matches standard DashMap RMW
    /// semantics. Phase-level atomicity lets us keep the cycle check
    /// outside the shard lock and avoid the walker-vs-DashMap deadlock
    /// that a lock-held closure would hit for self-referencing graphs.
    ///
    /// PHP counterpart: `Shared\Map::update($key, $fn): mixed`.
    ///
    /// [`set`]: MapInner::set
    /// [`remove`]: MapInner::remove
    pub fn update_with<F>(&self, key: Arc<str>, f: F) -> Result<Option<SharedValue>, SharedError>
    where
        F: FnOnce(Option<&SharedValue>) -> Option<SharedValue>,
    {
        let current = self.get(&key);
        let new_value = f(current.as_ref());

        match new_value {
            None => {
                // Remove if present; release the Map's hold on nested refs.
                let removed = self.remove(&key);
                if let (Some(reg), Some(prev)) = (REGISTRY.get(), &removed) {
                    sv_release_nested(prev, reg);
                }
                Ok(None)
            }
            Some(v) => {
                // set() covers cycle check, cap enforcement, retain, swap.
                let prev = self.set(key, v.clone())?;
                if let (Some(reg), Some(prev)) = (REGISTRY.get(), &prev) {
                    sv_release_nested(prev, reg);
                }
                Ok(Some(v))
            }
        }
    }

    /// Return the current value or compute-and-set via `factory`. The
    /// factory is called only when the key is missing. On a concurrent
    /// race where another thread inserts first, `factory`'s output is
    /// discarded and the existing value is returned.
    ///
    /// PHP counterpart: `Shared\Map::getOrSet($key, $factory): mixed`.
    pub fn get_or_set_with<F>(&self, key: Arc<str>, factory: F) -> Result<SharedValue, SharedError>
    where
        F: FnOnce() -> SharedValue,
    {
        if let Some(v) = self.get(&key) {
            return Ok(v);
        }
        let candidate = factory();
        match self.set_if_absent(Arc::clone(&key), candidate.clone())? {
            true => Ok(candidate),
            // Lost race: fall back to the winner's value, or to our
            // candidate if the key disappeared again (extreme race).
            false => Ok(self.get(&key).unwrap_or(candidate)),
        }
    }

    pub fn has(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    /// Remove and return the value, if any.
    ///
    /// Refcount contract (symmetric with [`set`]'s return path): the Map
    /// has dropped this key from its store but has NOT yet released the
    /// nested Shared retains — **caller MUST call [`sv_release_nested`]
    /// on the returned value once finished**. FFI paths first build the
    /// PHP wrapper (its own retain), then release.
    ///
    /// [`set`]: MapInner::set
    pub fn remove(&self, key: &str) -> Option<SharedValue> {
        let removed = self.entries.remove(key).map(|(_, v)| v);
        if removed.is_some() {
            self.count.fetch_sub(1, Ordering::AcqRel);
        }
        removed
    }

    pub fn count(&self) -> usize {
        self.count.load(Ordering::Acquire)
    }

    /// Drop every entry and release the Map's hold on each nested
    /// `SharedValue::Shared`. Terminal — there is no caller to inherit.
    pub fn clear(&self) {
        let reg = REGISTRY.get();
        self.entries.retain(|_, v| {
            if let Some(r) = reg {
                sv_release_nested(v, r);
            }
            false
        });
        // Resync count with DashMap's post-retain view (handles races
        // where a concurrent set slipped in during the retain walk).
        self.count.store(self.entries.len(), Ordering::Release);
    }

    /// Keep only entries matching the predicate. For each entry the
    /// Map drops (predicate returns `false`) the Map's hold on nested
    /// `SharedValue::Shared` targets is released — symmetric with
    /// [`clear`]: there is no caller to inherit the retain.
    ///
    /// Useful for TTL-style cleanup where the value carries its own
    /// expiry (timestamp encoded in the value) and a background task
    /// prunes stale entries in a single shard-walk instead of
    /// `keys().iter().filter().for_each(|k| remove(k))` which locks
    /// every shard twice.
    ///
    /// PHP counterpart: `Shared\Map::retain(fn($k, $v) => bool): int`
    /// (returns the number of entries kept).
    ///
    /// [`clear`]: MapInner::clear
    pub fn retain<F>(&self, mut f: F)
    where
        F: FnMut(&str, &SharedValue) -> bool,
    {
        let reg = REGISTRY.get();
        self.entries.retain(|k, v| {
            if f(k, v) {
                true
            } else {
                if let Some(r) = reg {
                    sv_release_nested(v, r);
                }
                false
            }
        });
        // Resync count: the predicate may have been called any number
        // of times, and concurrent writers may have slipped in during
        // the walk. DashMap's post-retain len() is the authoritative
        // post-state for this Map's entries field.
        self.count.store(self.entries.len(), Ordering::Release);
    }

    /// Snapshot of keys at call time. Iteration order is undefined
    /// (DashMap's shard order); user docs flag this.
    pub fn keys(&self) -> Vec<Arc<str>> {
        self.entries.iter().map(|e| Arc::clone(e.key())).collect()
    }
}

impl SharedInner for MapInner {
    fn type_tag(&self) -> SharedType {
        SharedType::Map
    }

    fn debug_snapshot(&self) -> SharedValue {
        // Per spec §mem_bytes: key count exposed as Long for
        // /__ox_shared/entry — users see entry.type_specific.key_count.
        SharedValue::Long(self.count() as i64)
    }

    fn mem_bytes(&self) -> usize {
        // Approximate per spec §mem_bytes — documented to drift ±30%
        // vs mallinfo. Per-entry ~64B (shard slot + Arc<str> + value
        // discriminant) + variable key & value sizes. +128B base
        // accounts for DashMap shard-array overhead.
        let count = self.count();
        let entries_bytes: usize = self
            .entries
            .iter()
            .map(|e| e.key().len() + 16 + e.value().mem_bytes())
            .sum();
        count * 64 + entries_bytes + 128
    }

    fn on_drop(&self) {
        // Map is being evicted from the registry. Walk every entry and
        // release the Map's hold on each nested `SharedValue::Shared`.
        // The registry outlives individual entries — this is called from
        // `SharedRegistry::release` which still holds the global static
        // alive.
        if let Some(reg) = REGISTRY.get() {
            for entry in self.entries.iter() {
                sv_release_nested(entry.value(), reg);
            }
        }
    }

    fn on_shutdown_notify(&self) {
        // Map operations don't block, so shutdown drain is a no-op.
    }

    fn children(&self, out: &mut Vec<SharedRef>) {
        // Expose every Shared ref reachable from any stored value.
        // The walker uses this to explore the reachability graph from
        // a root candidate toward a target (another Map's self_id).
        for entry in self.entries.iter() {
            collect_shared_refs(entry.value(), out);
        }
    }
}

// ─── FFI ──────────────────────────────────────────────────────────────────
//
// Signatures follow the 40-ffi-conventions.md §Batched array args decision:
// PHP-side serialises zval → portbuf bytes via `oxphp_portable_serialize`;
// Rust decodes with `portbuf_to_sv`. Returns travel the other direction via
// `sv_to_portbuf` + libc::malloc; PHP frees with `oxphp_portable_free`.
//
// Closure-driven ops (update / getOrSet) land in the next commit — they
// need the `zend_fcall_info_cache` shim.

use std::os::raw::c_int;

use crate::plugins::ox_shared::error::ffi_entry;
use crate::plugins::ox_shared::registry::registry;
use crate::plugins::ox_shared::value::{portbuf_to_sv, sv_to_portbuf};

/// Hand a `Vec<u8>` off to C via `libc::malloc`. Mirrors the pattern in
/// channel.rs so the C side uses a single `oxphp_portable_free` for all
/// Rust-allocated payload buffers.
///
/// # Safety
/// On success the caller owns the returned allocation; it must be freed
/// via `oxphp_portable_free` (which is just `libc::free` aliased).
unsafe fn payload_to_malloc(bytes: Vec<u8>) -> Result<(*mut u8, usize), SharedError> {
    let n = bytes.len();
    if n == 0 {
        return Ok((std::ptr::null_mut(), 0));
    }
    let ptr = unsafe { libc::malloc(n) as *mut u8 };
    if ptr.is_null() {
        set_last_error("libc::malloc failed for Map payload");
        return Err(SharedError::Generic);
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, n);
    }
    Ok((ptr, n))
}

/// Slurp a `(ptr, len)` pair into a UTF-8 `Arc<str>` key. Binary-safe
/// (accepts any bytes); non-UTF-8 input is replaced with lossy chars.
///
/// # Safety
/// `buf` must be valid for reads of `len` bytes when `len > 0`.
unsafe fn key_from_raw(buf: *const u8, len: usize) -> Result<Arc<str>, SharedError> {
    if len == 0 {
        return Ok(Arc::from(""));
    }
    if buf.is_null() {
        set_last_error("key buffer is null with non-zero length");
        return Err(SharedError::Generic);
    }
    let slice = unsafe { std::slice::from_raw_parts(buf, len) };
    Ok(match std::str::from_utf8(slice) {
        Ok(s) => Arc::from(s),
        Err(_) => Arc::from(String::from_utf8_lossy(slice).as_ref()),
    })
}

/// Create a new `Shared\Map`. `max_entries` ≤ 0 means unbounded.
///
/// # Safety
/// `out_id` must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_create(max_entries: i64, out_id: *mut u64) -> c_int {
    if out_id.is_null() {
        set_last_error("out_id is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let max = if max_entries > 0 {
            Some(max_entries as usize)
        } else {
            None
        };
        let reg = registry();
        let inner: Arc<dyn SharedInner> = Arc::new(MapInner::new(max));
        let id = reg.insert(SharedType::Map, Arc::clone(&inner))?;
        // Bind the Map's own id so cycle check activates on subsequent sets.
        (*inner)
            .as_any_map()
            .expect("just inserted MapInner")
            .bind_id(id);
        unsafe { *out_id = id };
        Ok(())
    })
}

/// # Safety
/// `out_count` must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_count(id: u64, out_count: *mut u64) -> c_int {
    if out_count.is_null() {
        set_last_error("out_count is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        reg.record_op(id);
        unsafe { *out_count = map.count() as u64 };
        Ok(())
    })
}

/// # Safety
/// `key` must be valid for reads of `klen` bytes (or `klen == 0`).
/// `out` must be valid for a `c_int` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_has(
    id: u64,
    key: *const u8,
    klen: usize,
    out: *mut c_int,
) -> c_int {
    if out.is_null() {
        set_last_error("out is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { key_from_raw(key, klen)? };
        reg.record_op(id);
        unsafe { *out = map.has(&k) as c_int };
        Ok(())
    })
}

#[no_mangle]
pub extern "C" fn oxphp_shared_map_clear(id: u64) -> c_int {
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        map.clear();
        reg.record_op(id);
        Ok(())
    })
}

/// Fetch a value by key. On success writes a malloc'd portbuf buffer
/// (callee frees via `oxphp_portable_free`). When the key is absent,
/// `*out_missing` is set to `1` and no buffer is allocated.
///
/// # Safety
/// `key` must be valid for reads of `klen` bytes (or `klen == 0`).
/// `out_buf`, `out_len`, `out_missing` must each be valid for writes.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_get(
    id: u64,
    key: *const u8,
    klen: usize,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_missing: *mut c_int,
) -> c_int {
    if out_buf.is_null() || out_len.is_null() || out_missing.is_null() {
        set_last_error("null out pointer");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_buf = std::ptr::null_mut();
        *out_len = 0;
        *out_missing = 0;
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { key_from_raw(key, klen)? };
        reg.record_op(id);
        match map.get(&k) {
            Some(v) => {
                let bytes = sv_to_portbuf(&v);
                let (ptr, n) = unsafe { payload_to_malloc(bytes)? };
                unsafe {
                    *out_buf = ptr;
                    *out_len = n;
                }
                Ok(())
            }
            None => {
                unsafe { *out_missing = 1 };
                Ok(())
            }
        }
    })
}

/// Store `value_buf` (portbuf-encoded) under `key`. Maps `SharedError::Cycle`
/// to status code `-9` and `SharedError::CapacityExceeded` to `-4`.
///
/// # Safety
/// `key` and `value_buf` must be valid for reads of `klen` / `vlen` bytes
/// respectively (or their `_len` counterpart `== 0`).
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_set(
    id: u64,
    key: *const u8,
    klen: usize,
    value_buf: *const u8,
    vlen: usize,
) -> c_int {
    if vlen > 0 && value_buf.is_null() {
        set_last_error("value_buf null with non-zero length");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { key_from_raw(key, klen)? };

        let value_bytes = if vlen == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(value_buf, vlen) }
        };
        let value = portbuf_to_sv(value_bytes)?;

        // Displaced value (if any) carries the Map's old retain per the
        // set() contract. FFI has no PHP wrapper to hand it to, so drop
        // the hold here.
        let prev = map.set(k, value)?;
        if let Some(prev) = prev {
            sv_release_nested(&prev, reg);
        }
        reg.record_op(id);
        Ok(())
    })
}

/// Atomic insert-if-absent. `*out_inserted` becomes `1` when the value
/// was stored, `0` when the key already existed.
///
/// # Safety
/// Same as `oxphp_shared_map_set`, plus `out_inserted` must be valid.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_set_if_absent(
    id: u64,
    key: *const u8,
    klen: usize,
    value_buf: *const u8,
    vlen: usize,
    out_inserted: *mut c_int,
) -> c_int {
    if out_inserted.is_null() {
        set_last_error("out_inserted is null");
        return SharedError::Generic.code();
    }
    if vlen > 0 && value_buf.is_null() {
        set_last_error("value_buf null with non-zero length");
        return SharedError::Generic.code();
    }
    unsafe { *out_inserted = 0 };
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { key_from_raw(key, klen)? };
        let value_bytes = if vlen == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(value_buf, vlen) }
        };
        let value = portbuf_to_sv(value_bytes)?;
        let inserted = map.set_if_absent(k, value)?;
        reg.record_op(id);
        unsafe { *out_inserted = inserted as c_int };
        Ok(())
    })
}

/// Remove a key. When present, writes a portbuf buffer of the previous
/// value (callee frees). When absent, sets `*out_missing = 1`.
///
/// # Safety
/// All `out_*` pointers must be valid for writes. `key` per klen.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_remove(
    id: u64,
    key: *const u8,
    klen: usize,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_missing: *mut c_int,
) -> c_int {
    if out_buf.is_null() || out_len.is_null() || out_missing.is_null() {
        set_last_error("null out pointer");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_buf = std::ptr::null_mut();
        *out_len = 0;
        *out_missing = 0;
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { key_from_raw(key, klen)? };
        match map.remove(&k) {
            Some(prev) => {
                // Build portbuf first (preserves SharedRef data) then drop
                // Map's hold on the nested Shared.
                let bytes = sv_to_portbuf(&prev);
                sv_release_nested(&prev, reg);
                let (ptr, n) = unsafe { payload_to_malloc(bytes)? };
                unsafe {
                    *out_buf = ptr;
                    *out_len = n;
                }
            }
            None => {
                unsafe { *out_missing = 1 };
            }
        }
        reg.record_op(id);
        Ok(())
    })
}

/// Bulk insert. `kv_buf` is a portbuf-encoded `SharedValue::Array` with
/// the string-keyed half holding the entries. Per-key atomicity (not
/// batch-atomic per spec §Atomic ops); bail at first error, with
/// `*out_inserted` holding the successful count at the bail point.
///
/// # Safety
/// `kv_buf` must be valid for reads of `kv_len` bytes (or `kv_len == 0`).
/// `out_inserted` must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_set_many(
    id: u64,
    kv_buf: *const u8,
    kv_len: usize,
    out_inserted: *mut u64,
) -> c_int {
    if out_inserted.is_null() {
        set_last_error("out_inserted is null");
        return SharedError::Generic.code();
    }
    if kv_len > 0 && kv_buf.is_null() {
        set_last_error("kv_buf null with non-zero length");
        return SharedError::Generic.code();
    }
    unsafe { *out_inserted = 0 };
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;

        let kv_bytes = if kv_len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(kv_buf, kv_len) }
        };
        let sv = portbuf_to_sv(kv_bytes)?;
        let arr = match sv {
            SharedValue::Array(a) => a,
            _ => {
                set_last_error("setMany expects an associative array");
                return Err(SharedError::Type);
            }
        };

        // Fold N per-key cycle walks into one via `set_many_batch`.
        // On partial failure the inner helper reports how many keys
        // landed before the bail, so the FFI contract (`out_inserted`
        // = successful count) still holds.
        //
        // `Arc::unwrap_or_clone` avoids re-allocating `Arc<SharedArray>`
        // when possible; portbuf_to_sv hands us a unique Arc here so the
        // clone fast-path kicks in.
        let pairs = Arc::try_unwrap(arr)
            .unwrap_or_else(|shared| (*shared).clone())
            .str_keyed;
        match map.set_many_batch(pairs) {
            Ok(n) => {
                unsafe { *out_inserted = n as u64 };
            }
            Err((e, n)) => {
                unsafe { *out_inserted = n as u64 };
                reg.record_op(id);
                return Err(e);
            }
        }
        reg.record_op(id);
        Ok(())
    })
}

/// Bulk fetch. `keys_buf` is a portbuf `SharedValue::Array` with string
/// values in the int-keyed half. Writes a portbuf of a keyed array with
/// per-key results (missing keys produce `Null` entries).
///
/// # Safety
/// `keys_buf` / `out_buf` / `out_len` must be valid per the usual
/// contract.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_get_many(
    id: u64,
    keys_buf: *const u8,
    keys_len: usize,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
) -> c_int {
    if out_buf.is_null() || out_len.is_null() {
        set_last_error("null out pointer");
        return SharedError::Generic.code();
    }
    if keys_len > 0 && keys_buf.is_null() {
        set_last_error("keys_buf null with non-zero length");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_buf = std::ptr::null_mut();
        *out_len = 0;
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;

        let keys_bytes = if keys_len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(keys_buf, keys_len) }
        };
        let sv = portbuf_to_sv(keys_bytes)?;
        let arr = match sv {
            SharedValue::Array(a) => a,
            _ => {
                set_last_error("getMany expects an array of keys");
                return Err(SharedError::Type);
            }
        };

        let mut out_arr = crate::plugins::ox_shared::value::SharedArray::default();
        for key_sv in &arr.int_keyed {
            let key_str: Arc<str> = match key_sv {
                SharedValue::String(s) => Arc::clone(s),
                SharedValue::Bytes(b) => Arc::from(String::from_utf8_lossy(b).as_ref()),
                _ => {
                    set_last_error("getMany keys must be strings");
                    return Err(SharedError::Type);
                }
            };
            let value = map.get(&key_str).unwrap_or(SharedValue::Null);
            out_arr.str_keyed.push((key_str, value));
        }
        reg.record_op(id);

        let result = SharedValue::Array(Arc::new(out_arr));
        let bytes = sv_to_portbuf(&result);
        let (ptr, n) = unsafe { payload_to_malloc(bytes)? };
        unsafe {
            *out_buf = ptr;
            *out_len = n;
        }
        Ok(())
    })
}

/// Bulk remove. `keys_buf` is a portbuf `SharedValue::Array` with string
/// values. Writes the number of keys actually removed (missing ones
/// don't count) into `*out_removed`.
///
/// # Safety
/// Per standard conventions for out pointers + keys buffer.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_remove_many(
    id: u64,
    keys_buf: *const u8,
    keys_len: usize,
    out_removed: *mut u64,
) -> c_int {
    if out_removed.is_null() {
        set_last_error("out_removed is null");
        return SharedError::Generic.code();
    }
    if keys_len > 0 && keys_buf.is_null() {
        set_last_error("keys_buf null with non-zero length");
        return SharedError::Generic.code();
    }
    unsafe { *out_removed = 0 };
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;

        let keys_bytes = if keys_len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(keys_buf, keys_len) }
        };
        let sv = portbuf_to_sv(keys_bytes)?;
        let arr = match sv {
            SharedValue::Array(a) => a,
            _ => {
                set_last_error("removeMany expects an array of keys");
                return Err(SharedError::Type);
            }
        };

        let mut removed: u64 = 0;
        for key_sv in &arr.int_keyed {
            let key_str: Arc<str> = match key_sv {
                SharedValue::String(s) => Arc::clone(s),
                SharedValue::Bytes(b) => Arc::from(String::from_utf8_lossy(b).as_ref()),
                _ => {
                    set_last_error("removeMany keys must be strings");
                    return Err(SharedError::Type);
                }
            };
            if let Some(prev) = map.remove(&key_str) {
                sv_release_nested(&prev, reg);
                removed += 1;
                unsafe { *out_removed = removed };
            }
        }
        reg.record_op(id);
        Ok(())
    })
}

/// Snapshot keys as a portbuf-encoded `SharedValue::Array` with string
/// entries in the `int_keyed` slot (so the PHP decoder produces a dense
/// numeric-indexed array). Iteration order is DashMap shard order (spec
/// `25-type-map.md §Iterator semantics` documents the undefined order).
///
/// # Safety
/// `out_buf`, `out_len` must be valid for writes.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_keys(
    id: u64,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
) -> c_int {
    if out_buf.is_null() || out_len.is_null() {
        set_last_error("null out pointer");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_buf = std::ptr::null_mut();
        *out_len = 0;
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        reg.record_op(id);

        let keys = map.keys();
        let mut arr = crate::plugins::ox_shared::value::SharedArray::default();
        for k in keys {
            arr.int_keyed.push(SharedValue::String(k));
        }
        let sv = SharedValue::Array(Arc::new(arr));
        let bytes = sv_to_portbuf(&sv);
        let (ptr, n) = unsafe { payload_to_malloc(bytes)? };
        unsafe {
            *out_buf = ptr;
            *out_len = n;
        }
        Ok(())
    })
}

// ─── Closure-driven FFI ─────────────────────────────────────────
//
// Uses the bridge's `oxphp_shared_invoke_*_portbuf` shims shared with
// Mutex::with. These serialise the closure's return into portbuf
// bytes so Rust doesn't need to touch zvals directly during
// invocation.

/// Read-modify-write. Fetches the current value (or `Null` if absent),
/// hands it to the PHP closure, stores whatever the closure returns —
/// `null` removes the key. Writes the stored value into `*out_buf`
/// (portbuf-encoded; `null`/absent after removal writes an empty buffer).
///
/// # Safety
/// `key` / `callable` / `out_buf` / `out_len` must be valid per usual
/// convention. `callable` must be a `zval *` on the invoking thread.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_update(
    id: u64,
    key: *const u8,
    klen: usize,
    callable: *mut std::os::raw::c_void,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
) -> c_int {
    if out_buf.is_null() || out_len.is_null() {
        set_last_error("null out pointer");
        return SharedError::Generic.code();
    }
    if callable.is_null() {
        set_last_error("callable is null");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_buf = std::ptr::null_mut();
        *out_len = 0;
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { key_from_raw(key, klen)? };

        // Snapshot current (or Null if absent) and serialise for the shim.
        let current = map.get(&k).unwrap_or(SharedValue::Null);
        let current_bytes = sv_to_portbuf(&current);

        let mut new_state_buf: *mut u8 = std::ptr::null_mut();
        let mut new_state_len: usize = 0;
        let mut ret_buf: *mut u8 = std::ptr::null_mut();
        let mut ret_len: usize = 0;
        let mut did_mutate: c_int = 0;

        let rc = unsafe {
            bridge_ffi::oxphp_shared_invoke_byref_1_portbuf(
                callable,
                current_bytes.as_ptr(),
                current_bytes.len(),
                &mut new_state_buf,
                &mut new_state_len,
                &mut ret_buf,
                &mut ret_len,
                &mut did_mutate,
            )
        };
        // The byref shim always writes `new_state_buf` on success;
        // Map::update only looks at the *return value*, not the mutated
        // arg. Free new_state_buf unconditionally.
        if !new_state_buf.is_null() {
            unsafe { bridge_ffi::oxphp_portable_free(new_state_buf) };
        }

        if rc == bridge_ffi::OXPHP_SHARED_INVOKE_PHP_THREW {
            if !ret_buf.is_null() {
                unsafe { bridge_ffi::oxphp_portable_free(ret_buf) };
            }
            // EG(exception) is already set on PHP side — surface via
            // Generic so the caller doesn't overwrite the engine exception.
            set_last_error("Map::update: closure threw");
            return Err(SharedError::Generic);
        }
        if rc != bridge_ffi::OXPHP_SHARED_INVOKE_OK {
            if !ret_buf.is_null() {
                unsafe { bridge_ffi::oxphp_portable_free(ret_buf) };
            }
            set_last_error("Map::update: invalid callable");
            return Err(SharedError::Type);
        }

        // Decode the closure's return.
        let new_bytes = if ret_buf.is_null() || ret_len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(ret_buf, ret_len).to_vec() }
        };
        if !ret_buf.is_null() {
            unsafe { bridge_ffi::oxphp_portable_free(ret_buf) };
        }
        let new_value = if new_bytes.is_empty() {
            SharedValue::Null
        } else {
            portbuf_to_sv(&new_bytes)?
        };

        // Apply: null → remove, else set.
        let stored = match new_value {
            SharedValue::Null => {
                if let Some(prev) = map.remove(&k) {
                    sv_release_nested(&prev, reg);
                }
                SharedValue::Null
            }
            v => {
                let prev = map.set(Arc::clone(&k), v.clone())?;
                if let Some(prev) = prev {
                    sv_release_nested(&prev, reg);
                }
                v
            }
        };
        reg.record_op(id);

        // Emit the stored value as portbuf so PHP side can decode directly.
        let out_bytes = sv_to_portbuf(&stored);
        let (ptr, n) = unsafe { payload_to_malloc(out_bytes)? };
        unsafe {
            *out_buf = ptr;
            *out_len = n;
        }
        Ok(())
    })
}

/// Return the current value or compute-and-store via the factory
/// closure. Factory is called only when the key is missing; on a
/// concurrent race the loser's output is discarded.
///
/// # Safety
/// Same as `oxphp_shared_map_update`.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_get_or_set(
    id: u64,
    key: *const u8,
    klen: usize,
    callable: *mut std::os::raw::c_void,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
) -> c_int {
    if out_buf.is_null() || out_len.is_null() {
        set_last_error("null out pointer");
        return SharedError::Generic.code();
    }
    if callable.is_null() {
        set_last_error("callable is null");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_buf = std::ptr::null_mut();
        *out_len = 0;
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { key_from_raw(key, klen)? };

        // Fast path: already present.
        if let Some(v) = map.get(&k) {
            reg.record_op(id);
            let bytes = sv_to_portbuf(&v);
            let (ptr, n) = unsafe { payload_to_malloc(bytes)? };
            unsafe {
                *out_buf = ptr;
                *out_len = n;
            }
            return Ok(());
        }

        // Slow path: call factory.
        let mut ret_buf: *mut u8 = std::ptr::null_mut();
        let mut ret_len: usize = 0;
        let rc = unsafe {
            bridge_ffi::oxphp_shared_invoke_0_portbuf(callable, &mut ret_buf, &mut ret_len)
        };
        if rc == bridge_ffi::OXPHP_SHARED_INVOKE_PHP_THREW {
            if !ret_buf.is_null() {
                unsafe { bridge_ffi::oxphp_portable_free(ret_buf) };
            }
            set_last_error("Map::getOrSet: factory threw");
            return Err(SharedError::Generic);
        }
        if rc != bridge_ffi::OXPHP_SHARED_INVOKE_OK {
            if !ret_buf.is_null() {
                unsafe { bridge_ffi::oxphp_portable_free(ret_buf) };
            }
            set_last_error("Map::getOrSet: invalid factory callable");
            return Err(SharedError::Type);
        }

        let candidate_bytes = if ret_buf.is_null() || ret_len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(ret_buf, ret_len).to_vec() }
        };
        if !ret_buf.is_null() {
            unsafe { bridge_ffi::oxphp_portable_free(ret_buf) };
        }
        let candidate = if candidate_bytes.is_empty() {
            SharedValue::Null
        } else {
            portbuf_to_sv(&candidate_bytes)?
        };

        let stored = match map.set_if_absent(Arc::clone(&k), candidate.clone())? {
            true => candidate,
            false => map.get(&k).unwrap_or(candidate),
        };
        reg.record_op(id);

        let bytes = sv_to_portbuf(&stored);
        let (ptr, n) = unsafe { payload_to_malloc(bytes)? };
        unsafe {
            *out_buf = ptr;
            *out_len = n;
        }
        Ok(())
    })
}

/// Apply `update_with` to every key in `keys_buf`. Writes a portbuf of
/// a keyed array (key → stored value) into `*out_buf`. Bail on first
/// error (per-key atomic, not batch-atomic).
///
/// # Safety
/// Per the usual conventions.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_update_many(
    id: u64,
    keys_buf: *const u8,
    keys_len: usize,
    callable: *mut std::os::raw::c_void,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
) -> c_int {
    if out_buf.is_null() || out_len.is_null() {
        set_last_error("null out pointer");
        return SharedError::Generic.code();
    }
    if callable.is_null() {
        set_last_error("callable is null");
        return SharedError::Generic.code();
    }
    if keys_len > 0 && keys_buf.is_null() {
        set_last_error("keys_buf null with non-zero length");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_buf = std::ptr::null_mut();
        *out_len = 0;
    }
    ffi_entry(|| {
        let reg = registry();
        let entry = reg.lookup(id)?;
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;

        let keys_bytes = if keys_len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(keys_buf, keys_len) }
        };
        let sv = portbuf_to_sv(keys_bytes)?;
        let arr = match sv {
            SharedValue::Array(a) => a,
            _ => {
                set_last_error("updateMany expects an array of keys");
                return Err(SharedError::Type);
            }
        };

        let mut out_arr = crate::plugins::ox_shared::value::SharedArray::default();
        for key_sv in &arr.int_keyed {
            let key_str: Arc<str> = match key_sv {
                SharedValue::String(s) => Arc::clone(s),
                SharedValue::Bytes(b) => Arc::from(String::from_utf8_lossy(b).as_ref()),
                _ => {
                    set_last_error("updateMany keys must be strings");
                    return Err(SharedError::Type);
                }
            };
            let current = map.get(&key_str).unwrap_or(SharedValue::Null);
            let cur_bytes = sv_to_portbuf(&current);

            let mut new_state_buf: *mut u8 = std::ptr::null_mut();
            let mut new_state_len: usize = 0;
            let mut ret_buf: *mut u8 = std::ptr::null_mut();
            let mut ret_len: usize = 0;
            let mut did_mutate: c_int = 0;

            let rc = unsafe {
                bridge_ffi::oxphp_shared_invoke_byref_1_portbuf(
                    callable,
                    cur_bytes.as_ptr(),
                    cur_bytes.len(),
                    &mut new_state_buf,
                    &mut new_state_len,
                    &mut ret_buf,
                    &mut ret_len,
                    &mut did_mutate,
                )
            };
            if !new_state_buf.is_null() {
                unsafe { bridge_ffi::oxphp_portable_free(new_state_buf) };
            }
            if rc == bridge_ffi::OXPHP_SHARED_INVOKE_PHP_THREW {
                if !ret_buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(ret_buf) };
                }
                set_last_error("Map::updateMany: closure threw");
                return Err(SharedError::Generic);
            }
            if rc != bridge_ffi::OXPHP_SHARED_INVOKE_OK {
                if !ret_buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(ret_buf) };
                }
                set_last_error("Map::updateMany: invalid callable");
                return Err(SharedError::Type);
            }
            let new_bytes = if ret_buf.is_null() || ret_len == 0 {
                Vec::new()
            } else {
                unsafe { std::slice::from_raw_parts(ret_buf, ret_len).to_vec() }
            };
            if !ret_buf.is_null() {
                unsafe { bridge_ffi::oxphp_portable_free(ret_buf) };
            }
            let new_value = if new_bytes.is_empty() {
                SharedValue::Null
            } else {
                portbuf_to_sv(&new_bytes)?
            };

            let stored = match new_value {
                SharedValue::Null => {
                    if let Some(prev) = map.remove(&key_str) {
                        sv_release_nested(&prev, reg);
                    }
                    SharedValue::Null
                }
                v => {
                    let prev = map.set(Arc::clone(&key_str), v.clone())?;
                    if let Some(prev) = prev {
                        sv_release_nested(&prev, reg);
                    }
                    v
                }
            };
            out_arr.str_keyed.push((key_str, stored));
        }
        reg.record_op(id);

        let result = SharedValue::Array(Arc::new(out_arr));
        let bytes = sv_to_portbuf(&result);
        let (ptr, n) = unsafe { payload_to_malloc(bytes)? };
        unsafe {
            *out_buf = ptr;
            *out_len = n;
        }
        Ok(())
    })
}

/// Downcast helper mirroring `SharedInnerCounterExt` — used by the FFI
/// functions above. Sound because `SharedType::Map` is only ever paired
/// with a `MapInner` allocation at the sole `oxphp_shared_map_create`
/// insertion site.
pub trait SharedInnerMapExt {
    fn as_any_map(&self) -> Option<&MapInner>;
}

impl SharedInnerMapExt for dyn SharedInner {
    fn as_any_map(&self) -> Option<&MapInner> {
        if self.type_tag() == SharedType::Map {
            // SAFETY: type_tag() == Map guarantees concrete type == MapInner
            // (insertion-site invariant). Casting a `*const dyn SharedInner`
            // to `*const MapInner` yields the data half of the fat pointer,
            // which is the address of the MapInner allocation.
            Some(unsafe { &*(self as *const dyn SharedInner as *const MapInner) })
        } else {
            None
        }
    }
}

// ─── PHP class registration ───────────────────────────────────────────────

use crate::bridge::ffi as bridge_ffi;
use crate::plugin::types::{MagicMethod, PhpType, PhpValue};
use crate::plugin::{PhpError, PluginContext, PluginError};
use crate::plugins::ox_shared::handle::SharedHandle;

/// Map `SharedError` FFI codes onto the `Shared\*` exception hierarchy.
/// Shared across all Map FFI dispatch paths.
fn map_rc_to_result(rc: c_int) -> Result<(), PhpError> {
    if rc == 0 {
        return Ok(());
    }
    let class = match rc {
        -2 => "OxPHP\\Shared\\StaleHandleException",
        -3 => "OxPHP\\Shared\\TypeException",
        -4 => "OxPHP\\Shared\\CapacityException",
        -9 => "OxPHP\\Shared\\CycleException",
        -10 => "OxPHP\\Shared\\UninitializedException",
        _ => "OxPHP\\Shared\\SharedException",
    };
    Err(PhpError::Exception {
        class: class.to_string(),
        message: read_last_error(),
        code: 0,
    })
}

fn read_last_error() -> String {
    let mut buf = [0u8; 512];
    let n = unsafe {
        crate::plugins::ox_shared::error::oxphp_shared_last_error(
            buf.as_mut_ptr() as *mut std::os::raw::c_char,
            buf.len(),
        )
    };
    let len = n.min(buf.len() - 1);
    String::from_utf8_lossy(&buf[..len]).into_owned()
}

/// Serialise arg `idx` (a `mixed` PHP value) to a libc-malloc'd portbuf
/// buffer. On success, caller owns the buffer and must free with
/// `oxphp_portable_free`.
///
/// Returns `TypeException` on any non-serialisable value (closures,
/// resources, etc.) — matches the spec for `Shared\Map::set`.
#[allow(clippy::type_complexity)]
fn serialize_mixed_arg(
    call: &mut crate::bridge::call::NativeCall,
    idx: u32,
    label: &str,
) -> Result<(*mut u8, usize), PhpError> {
    let arg_ptr = unsafe { call.raw_arg_ptr(idx) };
    let mut buf: *mut u8 = std::ptr::null_mut();
    let mut len: usize = 0;
    let rc =
        unsafe { bridge_ffi::oxphp_portable_serialize(arg_ptr as *const _, 1, &mut buf, &mut len) };
    if rc != 0 {
        if !buf.is_null() {
            unsafe { bridge_ffi::oxphp_portable_free(buf) };
        }
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".to_string(),
            message: format!("Shared\\Map::{label}: value is not serialisable (closure/resource)"),
            code: 0,
        });
    }
    Ok((buf, len))
}

/// Inverse: deserialise `(buf, len)` portbuf into `call`'s return-value
/// zval. Always frees `buf`. On decode failure, sets return to null.
fn deserialize_into_retval(call: &mut crate::bridge::call::NativeCall, buf: *mut u8, len: usize) {
    if buf.is_null() || len == 0 {
        call.ret_null();
        return;
    }
    let retval = call.retval_ptr();
    let rc = unsafe { bridge_ffi::oxphp_portable_deserialize(buf, len, 1, retval as *mut _) };
    unsafe { bridge_ffi::oxphp_portable_free(buf) };
    if rc != 0 {
        call.ret_null();
    }
}

pub fn register_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_class("OxPHP\\Shared\\Map")
        .implements("OxPHP\\Shared\\Shareable")
        .with_storage(|| SharedHandle::new(SharedType::Map))
        .magic(MagicMethod::Clone)
        .handler(|_call| {
            Err(PhpError::Exception {
                class: "OxPHP\\Shared\\SharedException".to_string(),
                message: "Shared\\Map instances cannot be cloned. Pass via \
                          oxphp_async(fn() use ($map) {...}) for cross-thread \
                          access, or construct a new instance."
                    .to_string(),
                code: 0,
            })
        })
        // ── __construct(?int $maxEntries = null) ───────────────────────
        .method("__construct")
        .optional_param(
            "maxEntries",
            PhpType::Nullable(Box::new(PhpType::Int)),
            PhpValue::Null,
        )
        .handler(|call| {
            let max_entries: i64 = if call.argc() > 0 && !call.arg_is_null(0).unwrap_or(true) {
                let n = call.arg_long(0).unwrap_or(-1);
                if n <= 0 {
                    return Err(PhpError::Exception {
                        class: "OxPHP\\Shared\\TypeException".to_string(),
                        message: "maxEntries must be a positive integer or null".to_string(),
                        code: 0,
                    });
                }
                n
            } else {
                -1
            };

            let mut out_id: u64 = 0;
            let rc = unsafe { oxphp_shared_map_create(max_entries, &mut out_id) };
            map_rc_to_result(rc)?;

            let h = call.storage_mut::<SharedHandle>()?;
            h.shared_id = out_id;
            h.type_tag = SharedType::Map as u8;
            Ok(())
        })
        // ── get(string $key, mixed $default = null): mixed ─────────────
        .method("get")
        .param("key", PhpType::String)
        .optional_param("default", PhpType::Mixed, PhpValue::Null)
        .returns(PhpType::Mixed)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let key = call.arg_str(0)?;
            let key_bytes = key.as_bytes();
            let mut buf: *mut u8 = std::ptr::null_mut();
            let mut len: usize = 0;
            let mut missing: c_int = 0;
            let rc = unsafe {
                oxphp_shared_map_get(
                    id,
                    key_bytes.as_ptr(),
                    key_bytes.len(),
                    &mut buf,
                    &mut len,
                    &mut missing,
                )
            };
            map_rc_to_result(rc)?;
            if missing != 0 {
                // Forward default (present or explicit null) unchanged.
                if call.argc() > 1 {
                    // Copy the default zval into RETVAL via bridge's deep copy.
                    let default_ptr = unsafe { call.raw_arg_ptr(1) };
                    unsafe {
                        bridge_ffi::oxphp_deep_copy_zval(
                            call.retval_ptr() as *mut _,
                            default_ptr as *const _,
                        );
                    }
                } else {
                    call.ret_null();
                }
                return Ok(());
            }
            deserialize_into_retval(call, buf, len);
            Ok(())
        })
        // ── set(string $key, mixed $value): void ───────────────────────
        .method("set")
        .param("key", PhpType::String)
        .param("value", PhpType::Mixed)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let key = call.arg_str(0)?.to_string();
            let (vbuf, vlen) = serialize_mixed_arg(call, 1, "set")?;

            let rc = unsafe { oxphp_shared_map_set(id, key.as_ptr(), key.len(), vbuf, vlen) };
            unsafe { bridge_ffi::oxphp_portable_free(vbuf) };
            map_rc_to_result(rc)?;
            call.ret_null();
            Ok(())
        })
        // ── has(string $key): bool ─────────────────────────────────────
        .method("has")
        .param("key", PhpType::String)
        .returns(PhpType::Bool)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let key = call.arg_str(0)?;
            let kb = key.as_bytes();
            let mut out: c_int = 0;
            let rc = unsafe { oxphp_shared_map_has(id, kb.as_ptr(), kb.len(), &mut out) };
            map_rc_to_result(rc)?;
            call.ret_bool(out != 0);
            Ok(())
        })
        // ── remove(string $key): mixed (prev or null) ──────────────────
        .method("remove")
        .param("key", PhpType::String)
        .returns(PhpType::Mixed)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let key = call.arg_str(0)?;
            let kb = key.as_bytes();
            let mut buf: *mut u8 = std::ptr::null_mut();
            let mut len: usize = 0;
            let mut missing: c_int = 0;
            let rc = unsafe {
                oxphp_shared_map_remove(id, kb.as_ptr(), kb.len(), &mut buf, &mut len, &mut missing)
            };
            map_rc_to_result(rc)?;
            if missing != 0 {
                call.ret_null();
                return Ok(());
            }
            deserialize_into_retval(call, buf, len);
            Ok(())
        })
        // ── clear(): void ──────────────────────────────────────────────
        .method("clear")
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let rc = oxphp_shared_map_clear(id);
            map_rc_to_result(rc)?;
            call.ret_null();
            Ok(())
        })
        // ── count(): int ───────────────────────────────────────────────
        .method("count")
        .returns(PhpType::Int)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let mut c: u64 = 0;
            let rc = unsafe { oxphp_shared_map_count(id, &mut c) };
            map_rc_to_result(rc)?;
            call.ret_long(c as i64);
            Ok(())
        })
        // ── keys(): array ──────────────────────────────────────────────
        .method("keys")
        .returns(PhpType::Array)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let mut buf: *mut u8 = std::ptr::null_mut();
            let mut len: usize = 0;
            let rc = unsafe { oxphp_shared_map_keys(id, &mut buf, &mut len) };
            map_rc_to_result(rc)?;
            // The payload is a portbuf-encoded SharedValue::Array — decode
            // straight into RETVAL (yields a dense PHP array).
            deserialize_into_retval(call, buf, len);
            Ok(())
        })
        // ── maxEntries(): ?int ─────────────────────────────────────────
        .method("maxEntries")
        .returns(PhpType::Nullable(Box::new(PhpType::Int)))
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let reg = crate::plugins::ox_shared::registry::registry();
            let entry = reg.lookup(id).map_err(|_| PhpError::Exception {
                class: "OxPHP\\Shared\\StaleHandleException".to_string(),
                message: "Map handle is no longer alive".to_string(),
                code: 0,
            })?;
            let map = entry
                .inner
                .as_any_map()
                .ok_or_else(|| PhpError::Exception {
                    class: "OxPHP\\Shared\\TypeException".to_string(),
                    message: "handle does not reference a Shared\\Map".to_string(),
                    code: 0,
                })?;
            match map.max_entries() {
                Some(n) => call.ret_long(n as i64),
                None => call.ret_null(),
            }
            Ok(())
        })
        // ── setIfAbsent(string $key, mixed $value): bool ───────────────
        .method("setIfAbsent")
        .param("key", PhpType::String)
        .param("value", PhpType::Mixed)
        .returns(PhpType::Bool)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let key = call.arg_str(0)?.to_string();
            let (vbuf, vlen) = serialize_mixed_arg(call, 1, "setIfAbsent")?;

            let mut inserted: c_int = 0;
            let rc = unsafe {
                oxphp_shared_map_set_if_absent(
                    id,
                    key.as_ptr(),
                    key.len(),
                    vbuf,
                    vlen,
                    &mut inserted,
                )
            };
            unsafe { bridge_ffi::oxphp_portable_free(vbuf) };
            map_rc_to_result(rc)?;
            call.ret_bool(inserted != 0);
            Ok(())
        })
        // ── update(string $key, callable $fn): mixed ───────────────────
        .method("update")
        .param("key", PhpType::String)
        .param("fn", PhpType::Callable)
        .returns(PhpType::Mixed)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let key = call.arg_str(0)?.to_string();
            let callable_zv = unsafe { call.raw_arg_ptr(1) };
            let mut buf: *mut u8 = std::ptr::null_mut();
            let mut len: usize = 0;
            let rc = unsafe {
                oxphp_shared_map_update(
                    id,
                    key.as_ptr(),
                    key.len(),
                    callable_zv,
                    &mut buf,
                    &mut len,
                )
            };
            // Generic (-1) here means the closure threw — EG(exception)
            // is already set; return Custom so the plugin wrapper doesn't
            // overwrite the engine exception.
            if rc == SharedError::Generic.code() {
                if !buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(buf) };
                }
                return Err(PhpError::Custom("Map::update closure threw".into()));
            }
            map_rc_to_result(rc)?;
            deserialize_into_retval(call, buf, len);
            Ok(())
        })
        // ── getOrSet(string $key, callable $factory): mixed ────────────
        .method("getOrSet")
        .param("key", PhpType::String)
        .param("factory", PhpType::Callable)
        .returns(PhpType::Mixed)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let key = call.arg_str(0)?.to_string();
            let callable_zv = unsafe { call.raw_arg_ptr(1) };
            let mut buf: *mut u8 = std::ptr::null_mut();
            let mut len: usize = 0;
            let rc = unsafe {
                oxphp_shared_map_get_or_set(
                    id,
                    key.as_ptr(),
                    key.len(),
                    callable_zv,
                    &mut buf,
                    &mut len,
                )
            };
            if rc == SharedError::Generic.code() {
                if !buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(buf) };
                }
                return Err(PhpError::Custom("Map::getOrSet factory threw".into()));
            }
            map_rc_to_result(rc)?;
            deserialize_into_retval(call, buf, len);
            Ok(())
        })
        // ── setMany(array $kv): int ────────────────────────────────────
        .method("setMany")
        .param("kv", PhpType::Array)
        .returns(PhpType::Int)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let (buf, len) = serialize_mixed_arg(call, 0, "setMany")?;
            let mut inserted: u64 = 0;
            let rc = unsafe { oxphp_shared_map_set_many(id, buf, len, &mut inserted) };
            unsafe { bridge_ffi::oxphp_portable_free(buf) };
            map_rc_to_result(rc)?;
            call.ret_long(inserted as i64);
            Ok(())
        })
        // ── getMany(array $keys): array ────────────────────────────────
        .method("getMany")
        .param("keys", PhpType::Array)
        .returns(PhpType::Array)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let (buf, len) = serialize_mixed_arg(call, 0, "getMany")?;
            let mut out_buf: *mut u8 = std::ptr::null_mut();
            let mut out_len: usize = 0;
            let rc = unsafe { oxphp_shared_map_get_many(id, buf, len, &mut out_buf, &mut out_len) };
            unsafe { bridge_ffi::oxphp_portable_free(buf) };
            map_rc_to_result(rc)?;
            deserialize_into_retval(call, out_buf, out_len);
            Ok(())
        })
        // ── updateMany(array $keys, callable $fn): array ───────────────
        .method("updateMany")
        .param("keys", PhpType::Array)
        .param("fn", PhpType::Callable)
        .returns(PhpType::Array)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let (keys_buf, keys_len) = serialize_mixed_arg(call, 0, "updateMany")?;
            let callable_zv = unsafe { call.raw_arg_ptr(1) };
            let mut buf: *mut u8 = std::ptr::null_mut();
            let mut len: usize = 0;
            let rc = unsafe {
                oxphp_shared_map_update_many(
                    id,
                    keys_buf,
                    keys_len,
                    callable_zv,
                    &mut buf,
                    &mut len,
                )
            };
            unsafe { bridge_ffi::oxphp_portable_free(keys_buf) };
            if rc == SharedError::Generic.code() {
                if !buf.is_null() {
                    unsafe { bridge_ffi::oxphp_portable_free(buf) };
                }
                return Err(PhpError::Custom("Map::updateMany closure threw".into()));
            }
            map_rc_to_result(rc)?;
            deserialize_into_retval(call, buf, len);
            Ok(())
        })
        // ── removeMany(array $keys): int ───────────────────────────────
        .method("removeMany")
        .param("keys", PhpType::Array)
        .returns(PhpType::Int)
        .handler(|call| {
            let id = call.storage::<SharedHandle>()?.shared_id;
            let (buf, len) = serialize_mixed_arg(call, 0, "removeMany")?;
            let mut removed: u64 = 0;
            let rc = unsafe { oxphp_shared_map_remove_many(id, buf, len, &mut removed) };
            unsafe { bridge_ffi::oxphp_portable_free(buf) };
            map_rc_to_result(rc)?;
            call.ret_long(removed as i64);
            Ok(())
        })
        // ── id(): int ──────────────────────────────────────────────────
        .method("id")
        .returns(PhpType::Int)
        .handler(|call| {
            let h = call.storage::<SharedHandle>()?;
            if !h.is_initialized() {
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\UninitializedException".to_string(),
                    message: "uninitialised Shared\\Map wrapper".to_string(),
                    code: 0,
                });
            }
            call.ret_long(h.shared_id as i64);
            Ok(())
        })
        .build()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(s: &str) -> Arc<str> {
        Arc::from(s)
    }

    #[test]
    fn new_unbounded() {
        let m = MapInner::new(None);
        assert_eq!(m.count(), 0);
        assert_eq!(m.max_entries(), None);
    }

    #[test]
    fn new_with_cap() {
        let m = MapInner::new(Some(500));
        assert_eq!(m.max_entries(), Some(500));
    }

    #[test]
    fn set_new_key_returns_none() {
        let m = MapInner::new(None);
        assert!(m.set(k("a"), SharedValue::Long(1)).unwrap().is_none());
        assert_eq!(m.count(), 1);
    }

    #[test]
    fn set_replace_returns_previous() {
        let m = MapInner::new(None);
        m.set(k("a"), SharedValue::Long(1)).unwrap();
        let prev = m.set(k("a"), SharedValue::Long(2)).unwrap();
        assert!(matches!(prev, Some(SharedValue::Long(1))));
        assert_eq!(m.count(), 1);
    }

    #[test]
    fn get_hit_and_miss() {
        let m = MapInner::new(None);
        m.set(k("a"), SharedValue::Long(42)).unwrap();
        assert!(matches!(m.get("a"), Some(SharedValue::Long(42))));
        assert!(m.get("missing").is_none());
    }

    #[test]
    fn has_reflects_presence() {
        let m = MapInner::new(None);
        m.set(k("a"), SharedValue::Long(1)).unwrap();
        assert!(m.has("a"));
        assert!(!m.has("b"));
        m.remove("a");
        assert!(!m.has("a"));
    }

    #[test]
    fn remove_returns_prev_value() {
        let m = MapInner::new(None);
        m.set(k("a"), SharedValue::Long(1)).unwrap();
        assert!(matches!(m.remove("a"), Some(SharedValue::Long(1))));
        assert!(m.remove("a").is_none());
        assert_eq!(m.count(), 0);
    }

    #[test]
    fn clear_empties_the_map() {
        let m = MapInner::new(None);
        for i in 0..10 {
            m.set(Arc::from(format!("k{i}")), SharedValue::Long(i))
                .unwrap();
        }
        assert_eq!(m.count(), 10);
        m.clear();
        assert_eq!(m.count(), 0);
        assert!(m.get("k0").is_none());
    }

    #[test]
    fn keys_returns_snapshot() {
        let m = MapInner::new(None);
        m.set(k("alpha"), SharedValue::Long(1)).unwrap();
        m.set(k("beta"), SharedValue::Long(2)).unwrap();
        m.set(k("gamma"), SharedValue::Long(3)).unwrap();

        let mut keys: Vec<String> = m.keys().iter().map(|s| s.to_string()).collect();
        keys.sort();
        assert_eq!(keys, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn keys_snapshot_is_independent_of_later_writes() {
        let m = MapInner::new(None);
        m.set(k("a"), SharedValue::Long(1)).unwrap();
        let snap = m.keys();
        m.set(k("b"), SharedValue::Long(2)).unwrap();
        assert_eq!(snap.len(), 1);
        assert_eq!(m.count(), 2);
    }

    #[test]
    fn shared_inner_impl_exposes_type_tag_and_snapshot() {
        let m = MapInner::new(Some(100));
        m.set(k("a"), SharedValue::Long(1)).unwrap();
        m.set(k("b"), SharedValue::Long(2)).unwrap();

        assert_eq!(m.type_tag(), SharedType::Map);
        match m.debug_snapshot() {
            SharedValue::Long(n) => assert_eq!(n, 2),
            other => panic!("expected Long, got {other:?}"),
        }
    }

    #[test]
    fn mem_bytes_scales_with_entries() {
        let m = MapInner::new(None);
        let empty = m.mem_bytes();
        for i in 0..50 {
            m.set(Arc::from(format!("key{i:03}")), SharedValue::Long(i))
                .unwrap();
        }
        assert!(m.mem_bytes() > empty);
    }

    #[test]
    fn downcast_ext_recovers_map_ref() {
        let m: Arc<dyn SharedInner> = Arc::new(MapInner::new(None));
        let concrete = (*m).as_any_map().expect("downcast should succeed");
        assert_eq!(concrete.count(), 0);
    }

    #[test]
    fn downcast_ext_rejects_wrong_type() {
        use crate::plugins::ox_shared::types::counter::CounterInner;
        let c: Arc<dyn SharedInner> = Arc::new(CounterInner::new(0));
        assert!((*c).as_any_map().is_none());
    }

    #[test]
    fn binary_safe_key_strings() {
        // Keys are raw bytes under the hood (Arc<str> accepts any UTF-8).
        // Non-ASCII but valid UTF-8 must round-trip.
        let m = MapInner::new(None);
        let key = k("ключ-π-🔥");
        m.set(key.clone(), SharedValue::Long(7)).unwrap();
        assert!(matches!(m.get(&key), Some(SharedValue::Long(7))));
    }

    // ── retain/release balance ────────────────────────────────────

    use crate::plugins::ox_shared::config::{LockDiagnosticsLevel, SharedConfig};
    use crate::plugins::ox_shared::registry::{init_registry, registry, SharedId, SharedRegistry};
    use crate::plugins::ox_shared::types::counter::CounterInner;
    use crate::plugins::ox_shared::value::{sv_release_nested, SharedRef};

    fn ensure_registry() -> &'static SharedRegistry {
        // Idempotent — OnceLock.set drops the dupe silently. Every test
        // that touches refcounts calls this; the first one wins.
        init_registry(SharedConfig {
            enabled: true,
            max_entries: 10_000,
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
        });
        registry()
    }

    /// Mint a fresh mock Shareable entry (a plain CounterInner) and return
    /// both the SharedValue wrapping it and the underlying id. The entry
    /// starts with ext_refcount = 1 (insert seed).
    fn make_mock_shared(reg: &SharedRegistry) -> (SharedValue, SharedId) {
        let id = reg
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .expect("registry capacity should be sufficient for test");
        let sv = SharedValue::Shared(SharedRef {
            id,
            type_tag: SharedType::Counter,
        });
        (sv, id)
    }

    #[test]
    fn set_shared_retains_target() {
        let reg = ensure_registry();
        let (sv, id) = make_mock_shared(reg);
        assert_eq!(reg.ext_refcount(id), Some(1));

        let m = MapInner::new(None);
        let prev = m.set(k("x"), sv).unwrap();
        assert!(prev.is_none());
        // Map's hold bumped refcount to 2.
        assert_eq!(reg.ext_refcount(id), Some(2));

        m.clear();
        assert_eq!(reg.ext_refcount(id), Some(1));
        reg.release(id);
        assert!(!reg.is_alive(id));
    }

    #[test]
    fn set_replace_transfers_prev_ownership_to_caller() {
        let reg = ensure_registry();
        let (sv1, id1) = make_mock_shared(reg);
        let (sv2, id2) = make_mock_shared(reg);

        let m = MapInner::new(None);
        m.set(k("x"), sv1).unwrap();
        assert_eq!(reg.ext_refcount(id1), Some(2));

        // Replace: new retained by set; prev returned with Map's retain
        // still outstanding — caller now owns that retain.
        let prev = m.set(k("x"), sv2).unwrap();
        let prev = prev.expect("replace should return prev");
        assert_eq!(
            reg.ext_refcount(id1),
            Some(2),
            "prev still carries Map's retain"
        );
        assert_eq!(reg.ext_refcount(id2), Some(2), "new retained by set");

        // Caller discharges the inherited retain.
        sv_release_nested(&prev, reg);
        assert_eq!(reg.ext_refcount(id1), Some(1));

        m.clear();
        assert_eq!(reg.ext_refcount(id2), Some(1));
        reg.release(id1);
        reg.release(id2);
    }

    #[test]
    fn remove_hands_retain_to_caller() {
        let reg = ensure_registry();
        let (sv, id) = make_mock_shared(reg);
        let m = MapInner::new(None);
        m.set(k("x"), sv).unwrap();
        assert_eq!(reg.ext_refcount(id), Some(2));

        let prev = m.remove("x").expect("key was present");
        // Map's store lost the key; the retain is carried by `prev`.
        assert_eq!(reg.ext_refcount(id), Some(2));

        sv_release_nested(&prev, reg);
        assert_eq!(reg.ext_refcount(id), Some(1));
        reg.release(id);
    }

    #[test]
    fn clear_releases_every_shared_entry() {
        let reg = ensure_registry();
        let (sv1, id1) = make_mock_shared(reg);
        let (sv2, id2) = make_mock_shared(reg);
        let m = MapInner::new(None);
        m.set(k("a"), sv1).unwrap();
        m.set(k("b"), sv2).unwrap();
        assert_eq!(reg.ext_refcount(id1), Some(2));
        assert_eq!(reg.ext_refcount(id2), Some(2));

        m.clear();
        assert_eq!(reg.ext_refcount(id1), Some(1));
        assert_eq!(reg.ext_refcount(id2), Some(1));
        reg.release(id1);
        reg.release(id2);
    }

    #[test]
    fn retain_keep_all_preserves_entries_and_refcounts() {
        let reg = ensure_registry();
        let (sv1, id1) = make_mock_shared(reg);
        let (sv2, id2) = make_mock_shared(reg);
        let m = MapInner::new(None);
        m.set(k("a"), sv1).unwrap();
        m.set(k("b"), sv2).unwrap();
        assert_eq!(reg.ext_refcount(id1), Some(2));
        assert_eq!(reg.ext_refcount(id2), Some(2));

        m.retain(|_, _| true);
        assert_eq!(m.count(), 2);
        assert!(m.has("a"));
        assert!(m.has("b"));
        assert_eq!(reg.ext_refcount(id1), Some(2));
        assert_eq!(reg.ext_refcount(id2), Some(2));

        m.clear();
        reg.release(id1);
        reg.release(id2);
    }

    #[test]
    fn retain_drop_all_empties_and_releases() {
        let reg = ensure_registry();
        let (sv1, id1) = make_mock_shared(reg);
        let (sv2, id2) = make_mock_shared(reg);
        let m = MapInner::new(None);
        m.set(k("a"), sv1).unwrap();
        m.set(k("b"), sv2).unwrap();

        m.retain(|_, _| false);
        assert_eq!(m.count(), 0);
        assert!(!m.has("a"));
        assert!(!m.has("b"));
        // Map's hold dropped; only the seed retain remains.
        assert_eq!(reg.ext_refcount(id1), Some(1));
        assert_eq!(reg.ext_refcount(id2), Some(1));
        reg.release(id1);
        reg.release(id2);
    }

    #[test]
    fn retain_partial_keeps_matched_drops_others() {
        let reg = ensure_registry();
        let (sv_keep, id_keep) = make_mock_shared(reg);
        let (sv_drop, id_drop) = make_mock_shared(reg);
        let m = MapInner::new(None);
        m.set(k("keep"), sv_keep).unwrap();
        m.set(k("drop"), sv_drop).unwrap();

        m.retain(|key, _| key == "keep");
        assert_eq!(m.count(), 1);
        assert!(m.has("keep"));
        assert!(!m.has("drop"));
        assert_eq!(reg.ext_refcount(id_keep), Some(2), "keep still held by Map");
        assert_eq!(reg.ext_refcount(id_drop), Some(1), "drop lost Map's hold");

        m.clear();
        reg.release(id_keep);
        reg.release(id_drop);
    }

    #[test]
    fn retain_predicate_sees_current_value() {
        let m = MapInner::new(None);
        m.set(k("a"), SharedValue::Long(1)).unwrap();
        m.set(k("b"), SharedValue::Long(2)).unwrap();
        m.set(k("c"), SharedValue::Long(3)).unwrap();

        m.retain(|_, v| matches!(v, SharedValue::Long(n) if *n >= 2));
        assert_eq!(m.count(), 2);
        assert!(!m.has("a"));
        assert!(m.has("b"));
        assert!(m.has("c"));
    }

    #[test]
    fn retain_on_empty_is_noop() {
        let m = MapInner::new(None);
        m.retain(|_, _| true);
        assert_eq!(m.count(), 0);
        m.retain(|_, _| false);
        assert_eq!(m.count(), 0);
    }

    #[test]
    fn retain_count_resyncs_after_walk() {
        // Retain must leave count() consistent with the internal DashMap
        // so subsequent cap checks / observability reads stay accurate.
        let m = MapInner::new(Some(10));
        for i in 0..8 {
            m.set(Arc::from(format!("k{i}")), SharedValue::Long(i))
                .unwrap();
        }
        assert_eq!(m.count(), 8);

        m.retain(|_, v| matches!(v, SharedValue::Long(n) if n % 2 == 0));
        assert_eq!(m.count(), 4);
        // Cap still has headroom: 10 − 4 = 6 new keys should fit.
        for i in 100..106 {
            m.set(Arc::from(format!("n{i}")), SharedValue::Long(i))
                .unwrap();
        }
        assert_eq!(m.count(), 10);
    }

    #[test]
    fn on_drop_releases_via_registry_release() {
        let reg = ensure_registry();
        let (sv, id) = make_mock_shared(reg);

        let inner: Arc<dyn SharedInner> = Arc::new(MapInner::new(None));
        // Bootstrap the Map entry *before* it owns anything — we need a
        // registry id to release in order to trigger on_drop.
        let map_id = reg.insert(SharedType::Map, Arc::clone(&inner)).unwrap();

        // Populate through the concrete type (via downcast).
        let map_concrete = (*inner).as_any_map().expect("just inserted MapInner");
        map_concrete.set(k("x"), sv).unwrap();
        assert_eq!(reg.ext_refcount(id), Some(2));

        // Drop the local Arc clone so registry.release drives refcount to 0.
        drop(inner);
        let final_count = reg.release(map_id);
        assert_eq!(final_count, 0, "Map entry evicted from registry");
        assert!(!reg.is_alive(map_id));

        // on_drop should have released Map's hold on the nested counter.
        assert_eq!(reg.ext_refcount(id), Some(1));
        reg.release(id);
    }

    #[test]
    fn shared_nested_inside_array_is_retained_and_released() {
        let reg = ensure_registry();
        let (sv_inner, id) = make_mock_shared(reg);
        assert_eq!(reg.ext_refcount(id), Some(1));

        // Build an array: ['key' => shared_ref]
        let mut arr = crate::plugins::ox_shared::value::SharedArray::default();
        arr.str_keyed.push((k("key"), sv_inner));
        let array_value = SharedValue::Array(Arc::new(arr));

        let m = MapInner::new(None);
        m.set(k("bucket"), array_value).unwrap();
        // Walker recursed into the array and retained the nested Counter.
        assert_eq!(reg.ext_refcount(id), Some(2));

        m.clear();
        assert_eq!(reg.ext_refcount(id), Some(1));
        reg.release(id);
    }

    // (The hot-path "scalar writes don't touch registry" guarantee is
    // enforced by the REGISTRY.get() gate + the scalar-branch early
    // exit in sv_retain_nested/release_nested — measured separately in
    // `benches/shared/map_scalar_throughput.rs`. A static total_entries
    // assertion here was fragile under parallel tests that populate
    // the shared registry.)

    // ── cycle detection integration ────────────────────────────────

    /// Bootstrap a Map into the registry and return (Arc<dyn SharedInner>, id).
    /// The returned Arc keeps a local clone alive so the caller can pass the
    /// value back in through downcast; caller must eventually drop it AND
    /// call `reg.release(id)` to trigger on_drop.
    fn bootstrap_map(reg: &SharedRegistry, max: Option<usize>) -> (Arc<dyn SharedInner>, SharedId) {
        let inner: Arc<dyn SharedInner> = Arc::new(MapInner::new(max));
        let id = reg.insert(SharedType::Map, Arc::clone(&inner)).unwrap();
        // Bind the self_id so cycle check activates.
        let concrete = (*inner).as_any_map().unwrap();
        concrete.bind_id(id);
        (inner, id)
    }

    #[test]
    fn direct_self_insert_is_rejected() {
        let reg = ensure_registry();
        let (map_arc, map_id) = bootstrap_map(reg, None);
        let map = (*map_arc).as_any_map().unwrap();

        let self_ref = SharedValue::Shared(SharedRef {
            id: map_id,
            type_tag: SharedType::Map,
        });
        let rc = map.set(k("loop"), self_ref);
        assert!(matches!(rc, Err(SharedError::Cycle)));
        // Nothing stored.
        assert_eq!(map.count(), 0);

        drop(map_arc);
        reg.release(map_id);
    }

    #[test]
    fn two_map_cycle_via_shared_is_rejected() {
        // a -> b (allowed), then b -> a (forms cycle).
        let reg = ensure_registry();
        let (a_arc, a_id) = bootstrap_map(reg, None);
        let (b_arc, b_id) = bootstrap_map(reg, None);
        let a = (*a_arc).as_any_map().unwrap();
        let b = (*b_arc).as_any_map().unwrap();

        a.set(
            k("b"),
            SharedValue::Shared(SharedRef {
                id: b_id,
                type_tag: SharedType::Map,
            }),
        )
        .expect("first edge is fine");

        let rc = b.set(
            k("a"),
            SharedValue::Shared(SharedRef {
                id: a_id,
                type_tag: SharedType::Map,
            }),
        );
        assert!(matches!(rc, Err(SharedError::Cycle)));
        assert_eq!(b.count(), 0);
        // a's original edge to b survived.
        assert_eq!(a.count(), 1);

        a.clear();
        drop(a_arc);
        drop(b_arc);
        reg.release(a_id);
        reg.release(b_id);
    }

    #[test]
    fn cycle_detected_at_depth_five() {
        // Chain A → B → C → D → E. Attempting to close E → A must be
        // detected by the walker at depth 5 (well under the default
        // SHARED_CYCLE_DETECT_DEPTH of 16).
        let reg = ensure_registry();
        let (a_arc, a_id) = bootstrap_map(reg, None);
        let (b_arc, b_id) = bootstrap_map(reg, None);
        let (c_arc, c_id) = bootstrap_map(reg, None);
        let (d_arc, d_id) = bootstrap_map(reg, None);
        let (e_arc, e_id) = bootstrap_map(reg, None);

        let a = (*a_arc).as_any_map().unwrap();
        let b = (*b_arc).as_any_map().unwrap();
        let c = (*c_arc).as_any_map().unwrap();
        let d = (*d_arc).as_any_map().unwrap();
        let e = (*e_arc).as_any_map().unwrap();

        for (from, to_id) in [(a, b_id), (b, c_id), (c, d_id), (d, e_id)] {
            from.set(
                k("next"),
                SharedValue::Shared(SharedRef {
                    id: to_id,
                    type_tag: SharedType::Map,
                }),
            )
            .expect("chain edge is fine");
        }

        let rc = e.set(
            k("back_to_a"),
            SharedValue::Shared(SharedRef {
                id: a_id,
                type_tag: SharedType::Map,
            }),
        );
        assert!(matches!(rc, Err(SharedError::Cycle)));
        assert_eq!(e.count(), 0);

        a.clear();
        b.clear();
        c.clear();
        d.clear();
        drop(a_arc);
        drop(b_arc);
        drop(c_arc);
        drop(d_arc);
        drop(e_arc);
        reg.release(a_id);
        reg.release(b_id);
        reg.release(c_id);
        reg.release(d_id);
        reg.release(e_id);
    }

    #[test]
    fn concurrent_writers_no_lost_updates() {
        // Spec exit-criteria mirror: 8 threads each inserting 1000
        // distinct keys into the same Map; the total count must match
        // exactly 8 * 1000 (no drops, no double-counts).
        use std::sync::Arc as StdArc;
        use std::thread;

        let m: StdArc<MapInner> = StdArc::new(MapInner::new(None));
        let n_threads = 8usize;
        let per_thread = 1000usize;

        let mut handles = Vec::with_capacity(n_threads);
        for t in 0..n_threads {
            let m = StdArc::clone(&m);
            handles.push(thread::spawn(move || {
                for i in 0..per_thread {
                    m.set(
                        Arc::from(format!("t{t}-k{i:04}")),
                        SharedValue::Long(i as i64),
                    )
                    .expect("unbounded map cannot cap");
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(m.count(), n_threads * per_thread);
    }

    #[test]
    fn cycle_via_array_nested_reference_is_rejected() {
        // a -> b -> array{'self' => a}  (rejected at step 2)
        let reg = ensure_registry();
        let (a_arc, a_id) = bootstrap_map(reg, None);
        let (b_arc, b_id) = bootstrap_map(reg, None);
        let a = (*a_arc).as_any_map().unwrap();
        let b = (*b_arc).as_any_map().unwrap();

        a.set(
            k("b"),
            SharedValue::Shared(SharedRef {
                id: b_id,
                type_tag: SharedType::Map,
            }),
        )
        .unwrap();

        // Try to insert array[self] = a into b — cycle via nested Shared.
        let mut arr = crate::plugins::ox_shared::value::SharedArray::default();
        arr.str_keyed.push((
            k("self"),
            SharedValue::Shared(SharedRef {
                id: a_id,
                type_tag: SharedType::Map,
            }),
        ));
        let rc = b.set(k("arr"), SharedValue::Array(Arc::new(arr)));
        assert!(matches!(rc, Err(SharedError::Cycle)));
        assert_eq!(b.count(), 0);

        a.clear();
        drop(a_arc);
        drop(b_arc);
        reg.release(a_id);
        reg.release(b_id);
    }

    #[test]
    fn unbound_map_skips_cycle_check() {
        // Without bind_id, the Map is invisible in the reachability graph;
        // even inserting a Shareable that "would" form a cycle is OK
        // (it can't because no one can reach this Map).
        let reg = ensure_registry();
        let (sv, id) = make_mock_shared(reg);

        let m = MapInner::new(None);
        // No bind_id → self_id unset.
        assert!(m.self_id().is_none());
        m.set(k("x"), sv).expect("unbound Map never sees cycles");

        m.clear();
        reg.release(id);
    }

    #[test]
    fn non_cyclic_shared_set_succeeds_and_stores() {
        // a.set('c', counter) where counter doesn't reach a — no cycle.
        let reg = ensure_registry();
        let (a_arc, a_id) = bootstrap_map(reg, None);
        let a = (*a_arc).as_any_map().unwrap();
        let (counter_sv, counter_id) = make_mock_shared(reg);

        a.set(k("c"), counter_sv).expect("no cycle");
        assert_eq!(a.count(), 1);
        assert_eq!(reg.ext_refcount(counter_id), Some(2));

        a.clear();
        drop(a_arc);
        reg.release(a_id);
        reg.release(counter_id);
    }

    #[test]
    fn cycle_error_sets_path_in_last_error() {
        use crate::plugins::ox_shared::error::{clear_last_error, oxphp_shared_last_error};
        use std::os::raw::c_char;

        let reg = ensure_registry();
        let (a_arc, a_id) = bootstrap_map(reg, None);
        let a = (*a_arc).as_any_map().unwrap();

        clear_last_error();
        let _ = a.set(
            k("self"),
            SharedValue::Shared(SharedRef {
                id: a_id,
                type_tag: SharedType::Map,
            }),
        );

        let mut buf = [0u8; 256];
        let len = unsafe { oxphp_shared_last_error(buf.as_mut_ptr() as *mut c_char, buf.len()) };
        assert!(len > 0);
        let msg = std::str::from_utf8(&buf[..len]).unwrap_or("");
        assert!(msg.contains("cycle"), "message should say cycle: {msg}");
        assert!(
            msg.contains(&format!("#{a_id}")),
            "message should include the Map id: {msg}"
        );

        drop(a_arc);
        reg.release(a_id);
    }

    // ── per-instance cap ───────────────────────────────────────────

    #[test]
    fn cap_rejects_new_key_when_full() {
        let m = MapInner::new(Some(3));
        m.set(k("k1"), SharedValue::Long(1)).unwrap();
        m.set(k("k2"), SharedValue::Long(2)).unwrap();
        m.set(k("k3"), SharedValue::Long(3)).unwrap();

        let rc = m.set(k("k4"), SharedValue::Long(4));
        assert!(matches!(rc, Err(SharedError::CapacityExceeded)));
        assert_eq!(m.count(), 3);
        assert!(m.get("k4").is_none());
    }

    #[test]
    fn cap_allows_overwrite_at_limit() {
        // Matches spec test_map_cap.php §overwrite-existing-works.
        let m = MapInner::new(Some(2));
        m.set(k("a"), SharedValue::Long(1)).unwrap();
        m.set(k("b"), SharedValue::Long(2)).unwrap();

        // Overwrite existing — must succeed.
        m.set(k("a"), SharedValue::Long(100))
            .expect("overwrite should succeed at cap");
        assert!(matches!(m.get("a"), Some(SharedValue::Long(100))));
        assert_eq!(m.count(), 2);
    }

    #[test]
    fn unbounded_map_never_caps() {
        let m = MapInner::new(None);
        for i in 0..1_000 {
            m.set(Arc::from(format!("k{i}")), SharedValue::Long(i))
                .unwrap();
        }
        assert_eq!(m.count(), 1_000);
    }

    #[test]
    fn cap_error_message_contains_limit() {
        use crate::plugins::ox_shared::error::{clear_last_error, oxphp_shared_last_error};
        use std::os::raw::c_char;

        let m = MapInner::new(Some(1));
        m.set(k("a"), SharedValue::Long(1)).unwrap();
        clear_last_error();
        let _ = m.set(k("b"), SharedValue::Long(2));

        let mut buf = [0u8; 256];
        let len = unsafe { oxphp_shared_last_error(buf.as_mut_ptr() as *mut c_char, buf.len()) };
        let msg = std::str::from_utf8(&buf[..len]).unwrap_or("");
        assert!(msg.contains("capacity"), "msg: {msg}");
        assert!(msg.contains("1/1"), "msg should name limit: {msg}");
        assert!(msg.contains("maxEntries"), "msg should hint at knob: {msg}");
    }

    #[test]
    fn remove_frees_cap_slot() {
        let m = MapInner::new(Some(2));
        m.set(k("a"), SharedValue::Long(1)).unwrap();
        m.set(k("b"), SharedValue::Long(2)).unwrap();

        // Full — next new key rejected.
        assert!(matches!(
            m.set(k("c"), SharedValue::Long(3)),
            Err(SharedError::CapacityExceeded)
        ));

        m.remove("a");
        assert_eq!(m.count(), 1);

        // Freed slot accepts a new key.
        m.set(k("c"), SharedValue::Long(3))
            .expect("slot should be free after remove");
        assert_eq!(m.count(), 2);
    }

    #[test]
    fn clear_frees_all_cap_slots() {
        let m = MapInner::new(Some(3));
        m.set(k("a"), SharedValue::Long(1)).unwrap();
        m.set(k("b"), SharedValue::Long(2)).unwrap();
        m.set(k("c"), SharedValue::Long(3)).unwrap();
        assert_eq!(m.count(), 3);

        m.clear();
        assert_eq!(m.count(), 0);

        for i in 0..3 {
            m.set(Arc::from(format!("k{i}")), SharedValue::Long(i))
                .unwrap();
        }
        assert_eq!(m.count(), 3);
    }

    #[test]
    fn cap_enforced_under_concurrent_inserts() {
        use std::sync::Arc as StdArc;
        use std::thread;

        let m: StdArc<MapInner> = StdArc::new(MapInner::new(Some(100)));
        let mut handles = Vec::new();

        // 8 threads each attempting 50 unique inserts → 400 attempts,
        // only 100 should succeed, matching the cap exactly.
        for t in 0..8 {
            let m = StdArc::clone(&m);
            handles.push(thread::spawn(move || {
                let mut accepted = 0usize;
                for i in 0..50 {
                    match m.set(Arc::from(format!("t{t}-k{i}")), SharedValue::Long(i)) {
                        Ok(_) => accepted += 1,
                        Err(SharedError::CapacityExceeded) => {}
                        Err(other) => panic!("unexpected err: {other:?}"),
                    }
                }
                accepted
            }));
        }

        let total_accepted: usize = handles.into_iter().map(|h| h.join().unwrap()).sum();
        assert_eq!(total_accepted, 100);
        assert_eq!(m.count(), 100);
    }

    // ── PHP class registration ─────────────────────────────────────

    #[test]
    fn register_class_emits_expected_methods() {
        use crate::events::EventDispatcher;
        use crate::plugin::context::PluginDecoratorDef;
        use crate::plugin::handler::{PluginInternalHandler, PluginMetricsCollector};
        use crate::plugin::php::PluginNativeFunctionDef;
        use std::collections::HashMap;

        let mut dispatcher = EventDispatcher::new();
        let mut services: HashMap<String, Box<dyn std::any::Any + Send + Sync>> = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics_collectors: Vec<Box<dyn PluginMetricsCollector>> = Vec::new();
        let mut internal_routes: HashMap<String, Box<dyn PluginInternalHandler>> = HashMap::new();
        let mut internal_route_prefixes: Vec<(String, Box<dyn PluginInternalHandler>)> = Vec::new();
        let mut native_php_functions: Vec<PluginNativeFunctionDef> = Vec::new();
        let mut decorators: Vec<PluginDecoratorDef> = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();
        let mut core_flags = HashMap::new();

        {
            let mut ctx = PluginContext::new(
                "ox_shared".into(),
                "__oxp_shared_".into(),
                &mut dispatcher,
                &mut services,
                &mut config_values,
                &mut metrics_collectors,
                &mut internal_routes,
                &mut internal_route_prefixes,
                &mut native_php_functions,
                &mut decorators,
                &mut php_classes,
                &mut php_interfaces,
                &mut php_enums,
                &mut php_attributes,
                &mut php_functions,
                &mut core_flags,
            );
            register_class(&mut ctx).unwrap();
        }

        let map_class = php_classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Shared\\Map")
            .expect("Map class must be registered");

        // Spot-check the methods expected on the class.
        let methods: std::collections::HashSet<&str> =
            map_class.methods.iter().map(|m| m.name.as_str()).collect();
        for expected in [
            "__construct",
            "get",
            "set",
            "has",
            "remove",
            "clear",
            "count",
            "keys",
            "maxEntries",
            "setIfAbsent",
            "update",
            "getOrSet",
            "setMany",
            "getMany",
            "removeMany",
            "updateMany",
            "id",
        ] {
            assert!(
                methods.contains(expected),
                "missing method `{expected}` in Map class registration"
            );
        }

        // Confirms the Shareable marker wiring still flows.
        assert!(map_class
            .interfaces
            .iter()
            .any(|iface| iface == "OxPHP\\Shared\\Shareable"));
    }

    // ── FFI round-trip ────────────────────────────────────────────

    #[test]
    fn ffi_create_and_basic_cycle() {
        let reg = ensure_registry();
        let mut id: u64 = 0;
        let rc = unsafe { oxphp_shared_map_create(0, &mut id) };
        assert_eq!(rc, 0);
        assert!(reg.is_alive(id));

        // Count starts at 0.
        let mut count: u64 = 0;
        assert_eq!(unsafe { oxphp_shared_map_count(id, &mut count) }, 0);
        assert_eq!(count, 0);

        // has("x") = false.
        let x = b"x";
        let mut has: c_int = 0;
        assert_eq!(
            unsafe { oxphp_shared_map_has(id, x.as_ptr(), x.len(), &mut has) },
            0
        );
        assert_eq!(has, 0);

        // set("x", 42) via portbuf.
        let v = sv_to_portbuf(&SharedValue::Long(42));
        assert_eq!(
            unsafe { oxphp_shared_map_set(id, x.as_ptr(), x.len(), v.as_ptr(), v.len()) },
            0
        );

        // has + count.
        unsafe { oxphp_shared_map_has(id, x.as_ptr(), x.len(), &mut has) };
        assert_eq!(has, 1);
        unsafe { oxphp_shared_map_count(id, &mut count) };
        assert_eq!(count, 1);

        // get → portbuf → decode.
        let mut buf: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        let mut missing: c_int = 0;
        let rc = unsafe {
            oxphp_shared_map_get(id, x.as_ptr(), x.len(), &mut buf, &mut len, &mut missing)
        };
        assert_eq!(rc, 0);
        assert_eq!(missing, 0);
        assert!(!buf.is_null() && len > 0);
        let decoded = portbuf_to_sv(unsafe { std::slice::from_raw_parts(buf, len) }).unwrap();
        assert!(matches!(decoded, SharedValue::Long(42)));
        unsafe { libc::free(buf as *mut libc::c_void) };

        // clear + count.
        assert_eq!(oxphp_shared_map_clear(id), 0);
        unsafe { oxphp_shared_map_count(id, &mut count) };
        assert_eq!(count, 0);

        reg.release(id);
    }

    #[test]
    fn ffi_missing_get_sets_missing_flag() {
        let reg = ensure_registry();
        let mut id: u64 = 0;
        unsafe { oxphp_shared_map_create(0, &mut id) };

        let key = b"nope";
        let mut buf: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        let mut missing: c_int = 0;
        let rc = unsafe {
            oxphp_shared_map_get(
                id,
                key.as_ptr(),
                key.len(),
                &mut buf,
                &mut len,
                &mut missing,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(missing, 1);
        assert!(buf.is_null() && len == 0);

        reg.release(id);
    }

    #[test]
    fn ffi_set_with_cap_rejects() {
        let reg = ensure_registry();
        let mut id: u64 = 0;
        unsafe { oxphp_shared_map_create(1, &mut id) };

        let k1 = b"k1";
        let v = sv_to_portbuf(&SharedValue::Long(1));
        unsafe { oxphp_shared_map_set(id, k1.as_ptr(), k1.len(), v.as_ptr(), v.len()) };

        let k2 = b"k2";
        let rc = unsafe { oxphp_shared_map_set(id, k2.as_ptr(), k2.len(), v.as_ptr(), v.len()) };
        assert_eq!(rc, SharedError::CapacityExceeded.code());

        reg.release(id);
    }

    #[test]
    fn ffi_remove_returns_prev() {
        let reg = ensure_registry();
        let mut id: u64 = 0;
        unsafe { oxphp_shared_map_create(0, &mut id) };

        let k = b"gone";
        let v = sv_to_portbuf(&SharedValue::String(Arc::from("value")));
        unsafe { oxphp_shared_map_set(id, k.as_ptr(), k.len(), v.as_ptr(), v.len()) };

        let mut buf: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        let mut missing: c_int = 0;
        let rc = unsafe {
            oxphp_shared_map_remove(id, k.as_ptr(), k.len(), &mut buf, &mut len, &mut missing)
        };
        assert_eq!(rc, 0);
        assert_eq!(missing, 0);
        assert!(!buf.is_null() && len > 0);
        let decoded = portbuf_to_sv(unsafe { std::slice::from_raw_parts(buf, len) }).unwrap();
        match decoded {
            SharedValue::String(s) => assert_eq!(&*s, "value"),
            other => panic!("expected String, got {other:?}"),
        }
        unsafe { libc::free(buf as *mut libc::c_void) };

        // Second remove hits missing path.
        let rc = unsafe {
            oxphp_shared_map_remove(id, k.as_ptr(), k.len(), &mut buf, &mut len, &mut missing)
        };
        assert_eq!(rc, 0);
        assert_eq!(missing, 1);

        reg.release(id);
    }

    #[test]
    fn ffi_set_if_absent_race() {
        let reg = ensure_registry();
        let mut id: u64 = 0;
        unsafe { oxphp_shared_map_create(0, &mut id) };

        let k = b"once";
        let v = sv_to_portbuf(&SharedValue::Long(1));
        let mut inserted: c_int = -1;

        assert_eq!(
            unsafe {
                oxphp_shared_map_set_if_absent(
                    id,
                    k.as_ptr(),
                    k.len(),
                    v.as_ptr(),
                    v.len(),
                    &mut inserted,
                )
            },
            0
        );
        assert_eq!(inserted, 1);

        assert_eq!(
            unsafe {
                oxphp_shared_map_set_if_absent(
                    id,
                    k.as_ptr(),
                    k.len(),
                    v.as_ptr(),
                    v.len(),
                    &mut inserted,
                )
            },
            0
        );
        assert_eq!(inserted, 0, "second call must not overwrite");

        reg.release(id);
    }

    #[test]
    fn ffi_keys_returns_portbuf_array() {
        let reg = ensure_registry();
        let mut id: u64 = 0;
        unsafe { oxphp_shared_map_create(0, &mut id) };

        for name in ["alpha", "beta", "gamma"] {
            let v = sv_to_portbuf(&SharedValue::Long(1));
            unsafe {
                oxphp_shared_map_set(id, name.as_ptr(), name.len(), v.as_ptr(), v.len());
            }
        }

        let mut buf: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        assert_eq!(unsafe { oxphp_shared_map_keys(id, &mut buf, &mut len) }, 0);
        assert!(!buf.is_null() && len > 0);

        let sv = portbuf_to_sv(unsafe { std::slice::from_raw_parts(buf, len) }).unwrap();
        let arr = match sv {
            SharedValue::Array(a) => a,
            other => panic!("expected Array, got {other:?}"),
        };
        let mut collected: Vec<String> = arr
            .int_keyed
            .iter()
            .filter_map(|v| match v {
                SharedValue::String(s) => Some(s.to_string()),
                _ => None,
            })
            .collect();
        collected.sort();
        assert_eq!(collected, vec!["alpha", "beta", "gamma"]);
        unsafe { libc::free(buf as *mut libc::c_void) };

        reg.release(id);
    }

    // ── batched FFI round-trip ─────────────────────────────────────

    fn encode_str_keyed(pairs: &[(&str, SharedValue)]) -> Vec<u8> {
        let mut arr = crate::plugins::ox_shared::value::SharedArray::default();
        for (k, v) in pairs {
            arr.str_keyed.push((Arc::from(*k), v.clone()));
        }
        sv_to_portbuf(&SharedValue::Array(Arc::new(arr)))
    }

    fn encode_int_keyed_strings(keys: &[&str]) -> Vec<u8> {
        let mut arr = crate::plugins::ox_shared::value::SharedArray::default();
        for k in keys {
            arr.int_keyed.push(SharedValue::String(Arc::from(*k)));
        }
        sv_to_portbuf(&SharedValue::Array(Arc::new(arr)))
    }

    #[test]
    fn ffi_set_many_inserts_all_pairs() {
        let reg = ensure_registry();
        let mut id: u64 = 0;
        unsafe { oxphp_shared_map_create(0, &mut id) };

        let buf = encode_str_keyed(&[
            ("a", SharedValue::Long(1)),
            ("b", SharedValue::Long(2)),
            ("c", SharedValue::Long(3)),
        ]);
        let mut inserted: u64 = 0;
        let rc = unsafe { oxphp_shared_map_set_many(id, buf.as_ptr(), buf.len(), &mut inserted) };
        assert_eq!(rc, 0);
        assert_eq!(inserted, 3);

        let mut count: u64 = 0;
        unsafe { oxphp_shared_map_count(id, &mut count) };
        assert_eq!(count, 3);

        reg.release(id);
    }

    #[test]
    fn ffi_set_many_respects_cap_mid_batch() {
        let reg = ensure_registry();
        let mut id: u64 = 0;
        unsafe { oxphp_shared_map_create(2, &mut id) };

        let buf = encode_str_keyed(&[
            ("a", SharedValue::Long(1)),
            ("b", SharedValue::Long(2)),
            ("c", SharedValue::Long(3)), // trips cap
        ]);
        let mut inserted: u64 = 0;
        let rc = unsafe { oxphp_shared_map_set_many(id, buf.as_ptr(), buf.len(), &mut inserted) };
        // Per-key atomic, bail at first failure.
        assert_eq!(rc, SharedError::CapacityExceeded.code());
        assert_eq!(inserted, 2, "partial count reflects the two that landed");

        reg.release(id);
    }

    #[test]
    fn ffi_get_many_returns_keyed_array_with_nulls() {
        let reg = ensure_registry();
        let mut id: u64 = 0;
        unsafe { oxphp_shared_map_create(0, &mut id) };

        let seed = encode_str_keyed(&[("a", SharedValue::Long(10)), ("b", SharedValue::Long(20))]);
        let mut n: u64 = 0;
        unsafe { oxphp_shared_map_set_many(id, seed.as_ptr(), seed.len(), &mut n) };

        let keys_buf = encode_int_keyed_strings(&["a", "b", "missing"]);
        let mut out_buf: *mut u8 = std::ptr::null_mut();
        let mut out_len: usize = 0;
        let rc = unsafe {
            oxphp_shared_map_get_many(
                id,
                keys_buf.as_ptr(),
                keys_buf.len(),
                &mut out_buf,
                &mut out_len,
            )
        };
        assert_eq!(rc, 0);

        let sv = portbuf_to_sv(unsafe { std::slice::from_raw_parts(out_buf, out_len) }).unwrap();
        unsafe { libc::free(out_buf as *mut libc::c_void) };

        let arr = match sv {
            SharedValue::Array(a) => a,
            other => panic!("expected Array, got {other:?}"),
        };
        assert_eq!(arr.str_keyed.len(), 3);
        assert_eq!(&*arr.str_keyed[0].0, "a");
        assert!(matches!(arr.str_keyed[0].1, SharedValue::Long(10)));
        assert_eq!(&*arr.str_keyed[1].0, "b");
        assert!(matches!(arr.str_keyed[1].1, SharedValue::Long(20)));
        assert_eq!(&*arr.str_keyed[2].0, "missing");
        assert!(matches!(arr.str_keyed[2].1, SharedValue::Null));

        reg.release(id);
    }

    #[test]
    fn ffi_remove_many_counts_hits_only() {
        let reg = ensure_registry();
        let mut id: u64 = 0;
        unsafe { oxphp_shared_map_create(0, &mut id) };

        let seed = encode_str_keyed(&[
            ("a", SharedValue::Long(1)),
            ("b", SharedValue::Long(2)),
            ("c", SharedValue::Long(3)),
        ]);
        let mut n: u64 = 0;
        unsafe { oxphp_shared_map_set_many(id, seed.as_ptr(), seed.len(), &mut n) };

        let keys_buf = encode_int_keyed_strings(&["a", "c", "nope"]);
        let mut removed: u64 = 0;
        let rc = unsafe {
            oxphp_shared_map_remove_many(id, keys_buf.as_ptr(), keys_buf.len(), &mut removed)
        };
        assert_eq!(rc, 0);
        assert_eq!(removed, 2, "only existing keys should count");

        let mut count: u64 = 0;
        unsafe { oxphp_shared_map_count(id, &mut count) };
        assert_eq!(count, 1); // "b" survives.

        reg.release(id);
    }

    #[test]
    fn ffi_set_many_empty_noop() {
        let reg = ensure_registry();
        let mut id: u64 = 0;
        unsafe { oxphp_shared_map_create(0, &mut id) };

        let buf = encode_str_keyed(&[]);
        let mut inserted: u64 = 0;
        let rc = unsafe { oxphp_shared_map_set_many(id, buf.as_ptr(), buf.len(), &mut inserted) };
        assert_eq!(rc, 0);
        assert_eq!(inserted, 0);

        reg.release(id);
    }

    #[test]
    fn ffi_bound_map_rejects_self_insert() {
        let reg = ensure_registry();
        let mut id: u64 = 0;
        unsafe { oxphp_shared_map_create(0, &mut id) };

        // Self-reference payload: SharedValue::Shared(id, Map).
        let self_ref = SharedValue::Shared(SharedRef {
            id,
            type_tag: SharedType::Map,
        });
        let v = sv_to_portbuf(&self_ref);

        let k = b"loop";
        let rc = unsafe { oxphp_shared_map_set(id, k.as_ptr(), k.len(), v.as_ptr(), v.len()) };
        assert_eq!(rc, SharedError::Cycle.code());

        reg.release(id);
    }

    // ── atomic RMW (setIfAbsent / update / getOrSet) ──────────────

    #[test]
    fn set_if_absent_inserts_when_vacant() {
        let m = MapInner::new(None);
        let inserted = m.set_if_absent(k("a"), SharedValue::Long(1)).unwrap();
        assert!(inserted);
        assert!(matches!(m.get("a"), Some(SharedValue::Long(1))));
    }

    #[test]
    fn set_if_absent_noop_when_occupied() {
        let m = MapInner::new(None);
        m.set(k("a"), SharedValue::Long(1)).unwrap();
        let inserted = m.set_if_absent(k("a"), SharedValue::Long(99)).unwrap();
        assert!(!inserted);
        assert!(matches!(m.get("a"), Some(SharedValue::Long(1))));
    }

    #[test]
    fn set_if_absent_respects_cap() {
        let m = MapInner::new(Some(1));
        m.set(k("a"), SharedValue::Long(1)).unwrap();

        let rc = m.set_if_absent(k("b"), SharedValue::Long(2));
        assert!(matches!(rc, Err(SharedError::CapacityExceeded)));
        assert_eq!(m.count(), 1);

        // Existing-key setIfAbsent is cheap (no cap touched).
        let rc = m.set_if_absent(k("a"), SharedValue::Long(42));
        assert!(!rc.unwrap());
    }

    #[test]
    fn update_modifies_existing() {
        let m = MapInner::new(None);
        m.set(k("n"), SharedValue::Long(10)).unwrap();

        let ret = m
            .update_with(k("n"), |cur| match cur {
                Some(SharedValue::Long(v)) => Some(SharedValue::Long(v * 2)),
                _ => Some(SharedValue::Long(0)),
            })
            .unwrap();
        assert!(matches!(ret, Some(SharedValue::Long(20))));
        assert!(matches!(m.get("n"), Some(SharedValue::Long(20))));
    }

    #[test]
    fn update_inserts_new_when_absent() {
        let m = MapInner::new(None);
        let ret = m
            .update_with(k("fresh"), |cur| {
                assert!(cur.is_none());
                Some(SharedValue::Long(42))
            })
            .unwrap();
        assert!(matches!(ret, Some(SharedValue::Long(42))));
        assert_eq!(m.count(), 1);
    }

    #[test]
    fn update_removes_on_none_return() {
        let m = MapInner::new(None);
        m.set(k("doomed"), SharedValue::Long(1)).unwrap();
        m.set(k("kept"), SharedValue::Long(2)).unwrap();

        let ret = m.update_with(k("doomed"), |_cur| None).unwrap();
        assert!(ret.is_none());
        assert!(m.get("doomed").is_none());
        assert_eq!(m.count(), 1);
        // Unrelated key untouched.
        assert!(matches!(m.get("kept"), Some(SharedValue::Long(2))));
    }

    #[test]
    fn update_noop_when_closure_returns_none_for_absent_key() {
        let m = MapInner::new(None);
        let ret = m.update_with(k("never"), |_cur| None).unwrap();
        assert!(ret.is_none());
        assert_eq!(m.count(), 0);
    }

    #[test]
    fn update_propagates_cycle_rejection() {
        let reg = ensure_registry();
        let (a_arc, a_id) = bootstrap_map(reg, None);
        let a = (*a_arc).as_any_map().unwrap();

        // Attempt to update key to a self-reference — cycle.
        let rc = a.update_with(k("self"), |_cur| {
            Some(SharedValue::Shared(SharedRef {
                id: a_id,
                type_tag: SharedType::Map,
            }))
        });
        assert!(matches!(rc, Err(SharedError::Cycle)));
        assert_eq!(a.count(), 0);

        drop(a_arc);
        reg.release(a_id);
    }

    #[test]
    fn get_or_set_returns_existing_value() {
        let m = MapInner::new(None);
        m.set(k("x"), SharedValue::Long(1)).unwrap();

        let called = std::cell::Cell::new(false);
        let got = m
            .get_or_set_with(k("x"), || {
                called.set(true);
                SharedValue::Long(999)
            })
            .unwrap();
        assert!(matches!(got, SharedValue::Long(1)));
        assert!(!called.get(), "factory must NOT run when key exists");
    }

    #[test]
    fn get_or_set_computes_when_absent() {
        let m = MapInner::new(None);
        let called = std::cell::Cell::new(false);
        let got = m
            .get_or_set_with(k("fresh"), || {
                called.set(true);
                SharedValue::Long(7)
            })
            .unwrap();
        assert!(matches!(got, SharedValue::Long(7)));
        assert!(called.get());
        assert!(matches!(m.get("fresh"), Some(SharedValue::Long(7))));
    }

    #[test]
    fn get_or_set_respects_cap() {
        let m = MapInner::new(Some(1));
        m.set(k("full"), SharedValue::Long(1)).unwrap();

        let rc = m.get_or_set_with(k("overflow"), || SharedValue::Long(99));
        assert!(matches!(rc, Err(SharedError::CapacityExceeded)));
    }

    #[test]
    fn set_if_absent_retain_released_on_cycle_rejection() {
        // Regression: cycle-rejected setIfAbsent must not leak retain counts
        // on the unrelated Shared that was part of the attempted value.
        let reg = ensure_registry();
        let (a_arc, a_id) = bootstrap_map(reg, None);
        let a = (*a_arc).as_any_map().unwrap();

        let rc = a.set_if_absent(
            k("loop"),
            SharedValue::Shared(SharedRef {
                id: a_id,
                type_tag: SharedType::Map,
            }),
        );
        assert!(matches!(rc, Err(SharedError::Cycle)));
        // `a`'s own ext_refcount unchanged (just the initial insert seed).
        assert_eq!(reg.ext_refcount(a_id), Some(1));

        drop(a_arc);
        reg.release(a_id);
    }

    #[test]
    fn no_mutation_on_rejected_cycle_insert() {
        // Verify the retain path is NOT run when cycle check fails —
        // otherwise the rejected value would leak a retain.
        let reg = ensure_registry();
        let (a_arc, a_id) = bootstrap_map(reg, None);
        let a = (*a_arc).as_any_map().unwrap();
        let (counter_sv_for_a, counter_id) = make_mock_shared(reg);

        a.set(k("c"), counter_sv_for_a).unwrap();
        assert_eq!(reg.ext_refcount(counter_id), Some(2));

        // Attempt self-insert (cycle): refcount of the *unrelated* counter
        // should not move.
        let rc = a.set(
            k("self"),
            SharedValue::Shared(SharedRef {
                id: a_id,
                type_tag: SharedType::Map,
            }),
        );
        assert!(matches!(rc, Err(SharedError::Cycle)));
        assert_eq!(
            reg.ext_refcount(counter_id),
            Some(2),
            "retain/release must not leak on rejected insert"
        );

        a.clear();
        drop(a_arc);
        reg.release(a_id);
        reg.release(counter_id);
    }
}
