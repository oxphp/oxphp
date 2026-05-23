//! `Shared\Map` — concurrent `MapKey → SharedValue` store.
//!
//! Built on DashMap with nested-Shareable lifetime via
//! `SharedValue::Shared(SharedRefOwned)`: every stored value carries
//! its own strong `Arc<Entry>` for any nested `Shared\*` it points to.
//! `set` moves the candidate value into storage (the Arc travels with
//! it). `clear` and `on_drop` simply drop the `SharedValue`s —
//! `SharedRefOwned::Drop` decrements the Arc.
//!
//! ## Null-as-absence invariant
//!
//! `null` is never a stored value: it is the absence sentinel across
//! the whole surface (`get`/`swap`/`pop`/`setIfAbsent` returning null
//! ⟺ the key was absent; `compareAndSet` treats `null` on either side
//! as "absent"). Writing `null` throws `TypeException`. This collapses
//! the mixed-return / `has()` apparatus exactly as
//! `java.util.concurrent.ConcurrentHashMap` and Go `sync.Map` do.
//!
//! ## Keys
//!
//! Keys are [`MapKey`] (`int | string`), kept **disjoint** — `123` and
//! `"123"` are different entries (no PHP key coercion).
//!
//! ## Counters & soft cap
//!
//! Entry count is tracked via **striped per-stripe counters**
//! (`Box<[AtomicIsize]>`), summed weakly-consistently by [`count`].
//! `maxEntries` is enforced against that sum and is therefore a **soft**
//! ceiling: under concurrent inserts the instance may overshoot by up to
//! ~(stripe count) before rejecting. Overwrites always succeed.
//!
//! ## Conditional mutation
//!
//! [`compare_and_set`] is the single linearisable primitive (insert /
//! replace / remove / no-op via the `None` absence sentinel on both
//! sides). [`swap`] / [`pop`] / [`set_if_absent`] return the prior
//! value. There is **no callback-under-lock** path — `forEach` re-fetches
//! each value with no shard lock held.
//!
//! [`MapKey`]: super::map_key::MapKey
//! [`count`]: MapInner::count
//! [`compare_and_set`]: MapInner::compare_and_set
//! [`swap`]: MapInner::swap
//! [`pop`]: MapInner::pop
//! [`set_if_absent`]: MapInner::set_if_absent

use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::{Arc, OnceLock, Weak};

use dashmap::DashMap;

use crate::plugins::ox_shared::cycle::{format_cycle_path, would_create_cycle, CycleError};
use crate::plugins::ox_shared::error::{read_last_error_message, set_last_error, SharedError};
use crate::plugins::ox_shared::registry::{
    Entry, SharedId, SharedInner, SharedType, ENTRY_MAGIC, REGISTRY,
};
use crate::plugins::ox_shared::types::map_key::MapKey;
use crate::plugins::ox_shared::value::{collect_shared_refs, raw_to_owned, SharedRef, SharedValue};

/// Number of striped per-stripe entry counters. A power of two so the
/// `hash % STRIPE_COUNT` mapping is cheap and well-distributed. The
/// stripe a key lands in is a *stable* function of the key — it need
/// not match DashMap's internal shard (correctness only requires
/// stability; cache locality is a non-goal here). The soft-cap overshoot
/// bound is at most this many entries under maximal write concurrency.
const STRIPE_COUNT: usize = 32;

/// Fixed key-slot cost for an `Int` key in [`map_entry_cost`]. `Str`
/// keys instead charge their byte length (mirroring the old `&str`
/// accounting). 8 bytes ≈ the `i64` payload.
const INT_KEY_COST: usize = 8;

/// Approximate byte-cost of a single `(key, value)` pair as accounted
/// in [`MapInner::mem_bytes`]: 64 B shard-slot + 16 B key overhead +
/// key-size + `value.mem_bytes()`. Used by mutator-site delta tracking
/// to keep `Entry::mem_bytes` and `total_bytes` in sync with container
/// growth without recomputing the full footprint on every op.
///
/// **Invariant — keep in sync with [`MapInner::mem_bytes`]**: the
/// per-entry portion (`64 + 16 + key-size + value.mem_bytes`) must match
/// the sum body there. The +128 base is booked separately by
/// `SharedRegistry::insert` and is NOT counted here.
fn map_entry_cost(key: &MapKey, value: &SharedValue) -> isize {
    map_entry_cost_parts(map_key_size(key), value.mem_bytes())
}

/// Parts-shaped twin of [`map_entry_cost`] — single source of truth for
/// the per-entry cost formula.
fn map_entry_cost_parts(key_size: usize, value_mem: usize) -> isize {
    (64 + 16 + key_size + value_mem) as isize
}

/// Size charged for a key in the mem-accounting: `Str` → byte length,
/// `Int` → [`INT_KEY_COST`].
fn map_key_size(key: &MapKey) -> usize {
    match key {
        MapKey::Int(_) => INT_KEY_COST,
        MapKey::Str(s) => s.len(),
    }
}

/// Rust-side storage for one `Shared\Map` instance.
pub struct MapInner {
    entries: DashMap<MapKey, SharedValue>,
    /// Per-instance cap; `None` = unbounded (subject only to the global
    /// `SHARED_MAX_ENTRIES`).
    max_entries: Option<usize>,
    /// Striped per-stripe entry counters (LongAdder-style). `count()`
    /// sums them; `max_entries` checks the sum. No global hot atomic on
    /// the write path, so writes scale across stripes. Weakly consistent:
    /// exact when quiescent, an approximation under concurrent writes.
    counts: Box<[AtomicIsize]>,
    /// The Map's own registry id, bound once by the creating FFI path
    /// via [`MapInner::bind_id`]. `None` before bind (or in Rust-only
    /// tests that skip registry insertion) — cycle detection is then
    /// a no-op because nothing in the registry can reach this Map.
    self_id: OnceLock<SharedId>,
    /// Cached `Weak<Entry>` bound by the creating FFI path via
    /// [`MapInner::bind_entry`]. Lets [`track_map_delta`] skip the
    /// registry shard-locked lookup.
    self_entry: OnceLock<Weak<Entry>>,
}

impl MapInner {
    pub fn new(max_entries: Option<usize>) -> Self {
        let counts = (0..STRIPE_COUNT)
            .map(|_| AtomicIsize::new(0))
            .collect::<Vec<_>>()
            .into_boxed_slice();
        Self {
            entries: DashMap::new(),
            max_entries,
            counts,
            self_id: OnceLock::new(),
            self_entry: OnceLock::new(),
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

    /// Bind this Map to its registry entry. Production path: call this
    /// from the creating FFI right after `registry.insert` with
    /// `Arc::downgrade(&entry_arc)`. Sets both the id (from the upgrade)
    /// and the cached `Weak<Entry>` that [`track_map_delta`] uses to
    /// bypass the registry shard-lock on every mutation.
    pub fn bind_entry(&self, weak: Weak<Entry>) {
        if let Some(arc) = weak.upgrade() {
            let _ = self.self_id.set(arc.id);
        }
        let _ = self.self_entry.set(weak);
    }

    pub fn self_id(&self) -> Option<SharedId> {
        self.self_id.get().copied()
    }

    /// Stable key → stripe index. Hashes the key with the default
    /// hasher and folds onto `STRIPE_COUNT`. A *stable* mapping is all
    /// that's required — it need not align with DashMap's internal shard.
    fn stripe_index(&self, key: &MapKey) -> usize {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut h);
        (h.finish() as usize) % self.counts.len()
    }

    fn inc_count(&self, key: &MapKey) {
        self.counts[self.stripe_index(key)].fetch_add(1, Ordering::Relaxed);
    }

    fn dec_count(&self, key: &MapKey) {
        self.counts[self.stripe_index(key)].fetch_sub(1, Ordering::Relaxed);
    }

    /// Adjust the registry's accounted memory by `delta`. Fast path uses
    /// the cached `Weak<Entry>`; falls back to the id-based slow path for
    /// fixtures that only ran [`bind_id`]. No-op when the Map is not yet
    /// registered or the registry has been torn down.
    fn track_map_delta(&self, delta: isize) {
        if delta == 0 {
            return;
        }
        if let Some(weak) = self.self_entry.get() {
            if let Some(entry) = weak.upgrade() {
                entry.adjust_mem_bytes(delta);
            }
            return;
        }
        let Some(id) = self.self_id.get().copied() else {
            return;
        };
        let Some(reg) = REGISTRY.get() else {
            return;
        };
        reg.adjust_mem_bytes(id, delta);
    }

    // ── Shared validation helpers ──────────────────────────────────
    //
    // `set` / `compare_and_set` / `swap` / `set_if_absent` all funnel
    // a candidate value through the same two checks before it enters
    // storage: (1) cycle detection on any nested Shareable, (2) the
    // value-type/null rejection. Factoring them here keeps the four
    // mutators consistent (issue: value-type rejection drift).

    /// Reject values that may never be stored: `null` is the absence
    /// sentinel, never a value. Objects/closures/resources never reach
    /// here as `SharedValue` (the portbuf codec / PHP-side serializer
    /// rejects them earlier), but `Null` can, so guard it explicitly.
    fn reject_unstorable(value: &SharedValue) -> Result<(), SharedError> {
        if matches!(value, SharedValue::Null) {
            set_last_error(
                "Shared\\Map: null is not a storable value (null means absence; \
                 use remove() / key absence instead)",
            );
            return Err(SharedError::Type);
        }
        Ok(())
    }

    /// Run the cycle walker for every `SharedValue::Shared` reachable
    /// from `value`. No-op if this Map has no `self_id` bound or the
    /// registry is absent.
    fn cycle_check(&self, value: &SharedValue) -> Result<(), SharedError> {
        let Some(reg) = REGISTRY.get() else {
            return Ok(());
        };
        self.check_cycles(reg, value)
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

    // ── Core mutators ──────────────────────────────────────────────

    /// Insert or replace. Returns the previous value, if any.
    ///
    /// Runs `reject_unstorable` + cycle check BEFORE any mutation so a
    /// rejected insert leaves the Map untouched. The candidate `value`
    /// is moved into storage on success — any nested `Shared` it carries
    /// brings its own `Arc<Entry>` along. The returned prev (if any)
    /// transfers its strong references to the caller.
    pub fn set(&self, key: MapKey, value: SharedValue) -> Result<Option<SharedValue>, SharedError> {
        Self::reject_unstorable(&value)?;
        self.cycle_check(&value)?;

        let new_size = value.mem_bytes() as isize;
        match self.entries.entry(key) {
            dashmap::Entry::Occupied(mut occ) => {
                let prev = std::mem::replace(occ.get_mut(), value);
                self.track_map_delta(new_size - prev.mem_bytes() as isize);
                Ok(Some(prev))
            }
            dashmap::Entry::Vacant(vac) => {
                if let Some(max) = self.max_entries {
                    // Soft cap: checked against the striped sum (may
                    // overshoot by up to STRIPE_COUNT under concurrent
                    // inserts). Overwrites bypass this (handled above).
                    if self.count() >= max {
                        set_last_error(format!(
                            "Shared\\Map capacity exceeded: {max}/{max} entries; \
                             raise `new Shared\\Map(maxEntries: ...)` or remove \
                             keys first"
                        ));
                        return Err(SharedError::CapacityExceeded);
                    }
                }
                let delta = map_entry_cost(vac.key(), &value);
                self.inc_count(vac.key());
                vac.insert(value);
                self.track_map_delta(delta);
                Ok(None)
            }
        }
    }

    /// Core single-key insert without the cycle check. Used by
    /// [`set_many_batch`] after a single batched cycle check covers
    /// every value in the incoming batch. Still runs `reject_unstorable`
    /// + cap. Same ownership contract as [`set`].
    fn set_without_cycle_check(
        &self,
        key: MapKey,
        value: SharedValue,
    ) -> Result<Option<SharedValue>, SharedError> {
        Self::reject_unstorable(&value)?;
        let new_size = value.mem_bytes() as isize;
        match self.entries.entry(key) {
            dashmap::Entry::Occupied(mut occ) => {
                let prev = std::mem::replace(occ.get_mut(), value);
                self.track_map_delta(new_size - prev.mem_bytes() as isize);
                Ok(Some(prev))
            }
            dashmap::Entry::Vacant(vac) => {
                if let Some(max) = self.max_entries {
                    if self.count() >= max {
                        set_last_error(format!(
                            "Shared\\Map capacity exceeded: {max}/{max} entries; \
                             raise `new Shared\\Map(maxEntries: ...)` or remove \
                             keys first"
                        ));
                        return Err(SharedError::CapacityExceeded);
                    }
                }
                let delta = map_entry_cost(vac.key(), &value);
                self.inc_count(vac.key());
                vac.insert(value);
                self.track_map_delta(delta);
                Ok(None)
            }
        }
    }

    /// Batched insert. Runs the cycle walker **once** over every value
    /// in the batch, then replays the per-key insert loop without
    /// re-running cycle detection.
    ///
    /// On first error (cycle, capacity, type) previously-committed
    /// writes are kept (partial-apply, per-key semantics); the returned
    /// count reports how many landed before the bail.
    pub fn set_many_batch(
        &self,
        batch: Vec<(MapKey, SharedValue)>,
    ) -> Result<usize, (SharedError, usize)> {
        // 1. Single cycle check covering every Shared ref in the batch.
        if let Some(reg) = REGISTRY.get() {
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

        // 2. Per-key insert (cycle check already done). Reject + cap
        //    still enforced per key via set_without_cycle_check.
        let mut inserted = 0;
        for (k, v) in batch {
            match self.set_without_cycle_check(k, v) {
                Ok(prev) => {
                    drop(prev);
                    inserted += 1;
                }
                Err(e) => return Err((e, inserted)),
            }
        }
        Ok(inserted)
    }

    /// Atomic insert-if-absent. Returns the prior value (`Some`) or
    /// `None` when the insert happened (`None` ⟺ inserted). Cycle + cap
    /// + null checks run on the candidate before it enters storage.
    ///
    /// PHP counterpart: `Shared\Map::setIfAbsent($key, $value): mixed`.
    pub fn set_if_absent(
        &self,
        key: MapKey,
        value: SharedValue,
    ) -> Result<Option<SharedValue>, SharedError> {
        Self::reject_unstorable(&value)?;
        self.cycle_check(&value)?;

        match self.entries.entry(key) {
            dashmap::Entry::Occupied(occ) => Ok(Some(occ.get().clone())),
            dashmap::Entry::Vacant(vac) => {
                if let Some(max) = self.max_entries {
                    if self.count() >= max {
                        set_last_error(format!(
                            "Shared\\Map capacity exceeded: {max}/{max} entries; \
                             raise `new Shared\\Map(maxEntries: ...)` or remove \
                             keys first"
                        ));
                        return Err(SharedError::CapacityExceeded);
                    }
                }
                let delta = map_entry_cost(vac.key(), &value);
                self.inc_count(vac.key());
                vac.insert(value);
                self.track_map_delta(delta);
                Ok(None)
            }
        }
    }

    /// Atomic compare-and-set — the single linearisable conditional
    /// primitive. `None` = absence on both sides:
    /// - `(expected=None, new=Some)` → insert iff absent
    /// - `(expected=Some(A), new=Some(B))` → replace iff current == A
    /// - `(expected=Some(A), new=None)` → remove iff current == A
    /// - `(expected=None, new=None)` → no-op (returns true iff absent)
    ///
    /// Returns `true` iff the swap was applied. Equality is by content
    /// (see [`sv_content_eq`]). Cycle/cap/null checks apply to the `new`
    /// side. The whole compare+swap runs under a single shard lock
    /// (`DashMap::entry`), so it is atomic wrt other writers on the key.
    pub fn compare_and_set(
        &self,
        key: MapKey,
        expected: Option<SharedValue>,
        new: Option<SharedValue>,
    ) -> Result<bool, SharedError> {
        // Pre-validate `new` (null/unstorable + cycle). Cap is handled
        // below under the entry lock on the insert path.
        if let Some(v) = &new {
            Self::reject_unstorable(v)?;
            self.cycle_check(v)?;
        }

        match self.entries.entry(key) {
            dashmap::Entry::Occupied(mut occ) => {
                let matches = match &expected {
                    Some(e) => sv_content_eq(occ.get(), e),
                    None => false, // expected absent, but key present
                };
                if !matches {
                    return Ok(false);
                }
                match new {
                    Some(v) => {
                        let new_size = v.mem_bytes() as isize;
                        let old = std::mem::replace(occ.get_mut(), v);
                        self.track_map_delta(new_size - old.mem_bytes() as isize);
                        // old's Arc(s) released at end of scope.
                    }
                    None => {
                        let (k, old) = occ.remove_entry();
                        self.track_map_delta(-map_entry_cost(&k, &old));
                        self.dec_count(&k);
                    }
                }
                Ok(true)
            }
            dashmap::Entry::Vacant(vac) => {
                if expected.is_some() {
                    return Ok(false); // expected a value, found none
                }
                match new {
                    None => Ok(true), // absent → absent: no-op
                    Some(v) => {
                        if let Some(max) = self.max_entries {
                            if self.count() >= max {
                                set_last_error(format!(
                                    "Shared\\Map capacity exceeded: {max}/{max} entries; \
                                     raise `new Shared\\Map(maxEntries: ...)` or remove \
                                     keys first"
                                ));
                                return Err(SharedError::CapacityExceeded);
                            }
                        }
                        let delta = map_entry_cost(vac.key(), &v);
                        self.inc_count(vac.key());
                        vac.insert(v);
                        self.track_map_delta(delta);
                        Ok(true)
                    }
                }
            }
        }
    }

    /// Overwrite and return the previous value (`None` ⟺ was absent).
    /// Runs reject + cap + cycle check on the stored value.
    ///
    /// PHP counterpart: `Shared\Map::swap($key, $value): mixed`.
    pub fn swap(
        &self,
        key: MapKey,
        value: SharedValue,
    ) -> Result<Option<SharedValue>, SharedError> {
        Self::reject_unstorable(&value)?;
        self.cycle_check(&value)?;

        let new_size = value.mem_bytes() as isize;
        match self.entries.entry(key) {
            dashmap::Entry::Occupied(mut occ) => {
                let prev = std::mem::replace(occ.get_mut(), value);
                self.track_map_delta(new_size - prev.mem_bytes() as isize);
                Ok(Some(prev))
            }
            dashmap::Entry::Vacant(vac) => {
                if let Some(max) = self.max_entries {
                    if self.count() >= max {
                        set_last_error(format!(
                            "Shared\\Map capacity exceeded: {max}/{max} entries; \
                             raise `new Shared\\Map(maxEntries: ...)` or remove \
                             keys first"
                        ));
                        return Err(SharedError::CapacityExceeded);
                    }
                }
                let delta = map_entry_cost(vac.key(), &value);
                self.inc_count(vac.key());
                vac.insert(value);
                self.track_map_delta(delta);
                Ok(None)
            }
        }
    }

    /// Remove and return the value (`None` ⟺ was absent). The returned
    /// `SharedValue` carries the Map's former `Arc<Entry>` strong
    /// reference(s) — the caller now owns those Arcs.
    ///
    /// PHP counterpart: `Shared\Map::pop($key): mixed`.
    pub fn pop(&self, key: &MapKey) -> Option<SharedValue> {
        self.entries.remove(key).map(|(k, v)| {
            self.track_map_delta(-map_entry_cost(&k, &v));
            self.dec_count(&k);
            v
        })
    }

    pub fn get(&self, key: &MapKey) -> Option<SharedValue> {
        self.entries.get(key).map(|r| r.value().clone())
    }

    /// Remove and discard the value. Returns whether it existed. No
    /// value materialised for the caller.
    ///
    /// PHP counterpart: `Shared\Map::remove($key): bool`.
    pub fn remove(&self, key: &MapKey) -> bool {
        self.pop(key).is_some()
    }

    /// Weakly-consistent entry count: sum of the striped counters,
    /// clamped to `>= 0` (a stale undercount can momentarily go
    /// negative under concurrent remove/insert races).
    pub fn count(&self) -> usize {
        self.counts
            .iter()
            .map(|c| c.load(Ordering::Relaxed))
            .sum::<isize>()
            .max(0) as usize
    }

    /// Drop every entry. Each dropped `SharedValue` releases its own
    /// nested `Arc<Entry>` refs. Returns the number removed.
    pub fn clear(&self) -> usize {
        let mut total_delta: isize = 0;
        let mut removed: usize = 0;
        // Decrement the stripe counter for each removed entry *inside* the
        // retain closure — DashMap holds the entry's shard lock there, the
        // same lock inserts take, so this is ordered against a concurrent
        // `set`. A blanket `store(0)` would race: an insert that lands
        // after retain passed its shard but before/around the reset would
        // leave a live entry whose increment is wiped, desyncing count()
        // permanently.
        self.entries.retain(|k, v| {
            total_delta -= map_entry_cost(k, v);
            self.counts[self.stripe_index(k)].fetch_sub(1, Ordering::Relaxed);
            removed += 1;
            false
        });
        self.track_map_delta(total_delta);
        removed
    }

    /// Snapshot every key into an owned `Vec<MapKey>` in a single
    /// `entries.iter()` pass (O(n)). DashMap locks one physical shard at
    /// a time and yields owned-clonable keys; cloning a `MapKey` is an
    /// `i64` copy or `Arc` bump. The returned Vec holds **no value /
    /// Shareable** — a concurrent delete frees the entry immediately —
    /// and no lock is held once this returns, so `forEach` can re-fetch
    /// each value and invoke PHP per key without holding any map lock.
    pub fn all_keys(&self) -> Vec<MapKey> {
        self.entries.iter().map(|e| e.key().clone()).collect()
    }

    /// Sample up to `limit` keys as display strings for the
    /// introspection endpoint. `Int` keys render as their decimal form;
    /// iteration order is undefined (DashMap shard order). Not a public
    /// PHP surface — observability only.
    pub fn sample_keys(&self, limit: usize) -> Vec<String> {
        self.entries
            .iter()
            .take(limit)
            .map(|e| match e.key() {
                MapKey::Int(i) => i.to_string(),
                // Display only — lossy is fine for the introspection endpoint.
                MapKey::Str(s) => String::from_utf8_lossy(s).into_owned(),
            })
            .collect()
    }
}

impl SharedInner for MapInner {
    fn type_tag(&self) -> SharedType {
        SharedType::Map
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn debug_snapshot(&self) -> SharedValue {
        SharedValue::Long(self.count() as i64)
    }

    fn mem_bytes(&self) -> usize {
        // Approximate per spec §mem_bytes — documented to drift ±30%
        // vs mallinfo. Per-entry ~64B + key & value sizes. +128B base
        // accounts for DashMap shard-array overhead.
        let count = self.count();
        let entries_bytes: usize = self
            .entries
            .iter()
            .map(|e| map_key_size(e.key()) + 16 + e.value().mem_bytes())
            .sum();
        count * 64 + entries_bytes + 128
    }

    fn on_drop(&self) {
        // Map is being evicted. Every stored `SharedValue::Shared`
        // releases its `Arc<Entry>` when the DashMap drops the entry —
        // no explicit walk needed.
    }

    fn on_shutdown_notify(&self) {
        // Map operations don't block, so shutdown drain is a no-op.
    }

    fn children(&self, out: &mut Vec<SharedRef>) {
        for entry in self.entries.iter() {
            collect_shared_refs(entry.value(), out);
        }
    }
}

/// Content equality for `compareAndSet`. Scalars by value; nested
/// `Shareable` (`SharedValue::Shared`) by registry id (monotonic, never
/// reused — so a re-inserted entry that matches is genuinely the same
/// live entry); strings/arrays by `sv_to_portbuf` byte equality.
///
/// Array equality is element-content equality over the stored
/// representation, which keeps integer-keyed and string-keyed members in
/// separate runs. This matches PHP `===` for the usual cases (list arrays;
/// all-int or all-string keyed arrays), but NOT for arrays that interleave
/// int and string keys in a specific order — e.g. `[0=>'a','x'=>'b',1=>'c']`
/// and `[0=>'a',1=>'c','x'=>'b']` compare equal here though PHP `===`
/// distinguishes them. This is inherent to how the Map normalises stored
/// arrays (it would reorder them on read-back too), not a CAS-only quirk.
fn sv_content_eq(a: &SharedValue, b: &SharedValue) -> bool {
    use SharedValue::*;
    match (a, b) {
        (Null, Null) => true,
        (Bool(x), Bool(y)) => x == y,
        (Long(x), Long(y)) => x == y,
        (Double(x), Double(y)) => x == y,
        (Shared(x), Shared(y)) => x.id == y.id,
        // Strings/Bytes/Arrays (and any cross-string/bytes pair) compare
        // by serialised bytes. Mixed scalar pairs (e.g. Long vs Double)
        // serialise to different tags and so compare unequal.
        _ => sv_to_portbuf(a) == sv_to_portbuf(b),
    }
}

// ─── FFI ──────────────────────────────────────────────────────────────────
//
// Keys travel as a tagged tuple `(key_kind: c_int /*0=int,1=str*/,
// key_int: i64, key_ptr, key_len)`. `decode_map_key` reconstructs a
// `MapKey` from that tuple; every key-taking FFI fn uses it.
//
// Values travel as portbuf bytes: PHP serialises zval → portbuf via
// `oxphp_portable_serialize`; Rust decodes with `portbuf_to_sv` +
// `raw_to_owned`. Returns travel back via `sv_to_portbuf` + libc::malloc;
// PHP frees with `oxphp_portable_free`.

use std::os::raw::c_int;

use crate::plugins::ox_shared::error::ffi_entry;
use crate::plugins::ox_shared::registry::registry;
use crate::plugins::ox_shared::value::{portbuf_to_sv, sv_to_portbuf};

/// Key discriminant in the FFI tagged-key tuple.
const KEY_KIND_INT: c_int = 0;
const KEY_KIND_STR: c_int = 1;

/// Reconstruct a [`MapKey`] from the tagged FFI key tuple.
///
/// # Safety
/// When `kind == KEY_KIND_STR`, `ptr` must be valid for reads of `len`
/// bytes (or `len == 0`).
unsafe fn decode_map_key(
    kind: c_int,
    key_int: i64,
    ptr: *const u8,
    len: usize,
) -> Result<MapKey, SharedError> {
    match kind {
        KEY_KIND_INT => Ok(MapKey::Int(key_int)),
        KEY_KIND_STR => {
            // String keys are binary-safe: store the raw bytes opaquely so a
            // non-UTF-8 PHP string key round-trips faithfully.
            if len == 0 {
                return Ok(MapKey::Str(Arc::from(&[][..])));
            }
            if ptr.is_null() {
                set_last_error("key buffer is null with non-zero length");
                return Err(SharedError::Generic);
            }
            let slice = unsafe { std::slice::from_raw_parts(ptr, len) };
            Ok(MapKey::Str(Arc::from(slice)))
        }
        _ => {
            set_last_error("invalid map key kind (expected 0=int or 1=str)");
            Err(SharedError::Type)
        }
    }
}

/// Hand a `Vec<u8>` off to C via `libc::malloc`. Mirrors channel.rs so
/// the C side uses a single `oxphp_portable_free` for all Rust-allocated
/// payload buffers.
///
/// # Safety
/// On success the caller owns the returned allocation; free via
/// `oxphp_portable_free`.
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

/// Decode a value portbuf slice into an owning `SharedValue`.
fn decode_value(
    bytes: &[u8],
    reg: &'static crate::plugins::ox_shared::registry::SharedRegistry,
) -> Result<SharedValue, SharedError> {
    let raw = portbuf_to_sv(bytes)?;
    raw_to_owned(raw, reg)
}

/// Create a new `Shared\Map`. `max_entries` ≤ 0 means unbounded.
///
/// # Safety
/// `out_ptr` must be valid for writes of `*const Entry` if non-null.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_create(
    max_entries: i64,
    out_ptr: *mut *const Entry,
) -> c_int {
    if out_ptr.is_null() {
        set_last_error("out_ptr is null");
        return SharedError::Generic.code();
    }
    ffi_entry(|| {
        let max = if max_entries > 0 {
            Some(max_entries as usize)
        } else {
            None
        };
        let reg = registry();
        let typed = Arc::new(MapInner::new(max));
        let arc = reg.insert(SharedType::Map, typed.clone())?;
        typed.bind_entry(Arc::downgrade(&arc));
        // SAFETY: out_ptr checked non-null above.
        unsafe { *out_ptr = Arc::into_raw(arc) };
        Ok(())
    })
}

/// # Safety
/// `out_count` must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_count(
    entry_ptr: *const Entry,
    out_count: *mut u64,
) -> c_int {
    if out_count.is_null() {
        set_last_error("out_count is null");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "map_count on freed Entry");
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        entry.registry.record_op(entry);
        unsafe { *out_count = map.count() as u64 };
        Ok(())
    })
}

/// # Safety
/// `entry_ptr` must be a live `Arc::into_raw` pointer or NULL.
/// `out_removed` must be valid for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_clear(
    entry_ptr: *const Entry,
    out_removed: *mut u64,
) -> c_int {
    if out_removed.is_null() {
        set_last_error("out_removed is null");
        return SharedError::Generic.code();
    }
    unsafe { *out_removed = 0 };
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "map_clear on freed Entry");
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let removed = map.clear();
        entry.registry.record_op(entry);
        unsafe { *out_removed = removed as u64 };
        Ok(())
    })
}

/// Fetch a value by key. On success writes a malloc'd portbuf buffer
/// (callee frees via `oxphp_portable_free`). When the key is absent,
/// `*out_absent` is set to `1` and no buffer is allocated.
///
/// # Safety
/// Key tuple per `decode_map_key`. `out_buf`, `out_len`, `out_absent`
/// must each be valid for writes.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn oxphp_shared_map_get(
    entry_ptr: *const Entry,
    key_kind: c_int,
    key_int: i64,
    key_ptr: *const u8,
    key_len: usize,
    out_buf: *mut *mut u8,
    out_len: *mut usize,
    out_absent: *mut c_int,
) -> c_int {
    if out_buf.is_null() || out_len.is_null() || out_absent.is_null() {
        set_last_error("null out pointer");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_buf = std::ptr::null_mut();
        *out_len = 0;
        *out_absent = 0;
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "map_get on freed Entry");
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { decode_map_key(key_kind, key_int, key_ptr, key_len)? };
        entry.registry.record_op(entry);
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
                unsafe { *out_absent = 1 };
                Ok(())
            }
        }
    })
}

/// Reject a value whose serialised size exceeds the configured per-value
/// cap (`SHARED_MAX_VALUE_SIZE`, default 1 MiB). Guards against an
/// allocation bomb from PHP-side input.
fn enforce_value_size(
    registry: &crate::plugins::ox_shared::registry::SharedRegistry,
    serialized_len: usize,
) -> Result<(), SharedError> {
    let cap = registry.config().max_value_size;
    if serialized_len > cap {
        set_last_error(format!(
            "Shared\\Map value of {serialized_len} bytes exceeds the per-value \
             cap of {cap} bytes; raise SHARED_MAX_VALUE_SIZE or store less"
        ));
        return Err(SharedError::ValueTooLarge);
    }
    Ok(())
}

/// Store `value_buf` (portbuf-encoded) under `key`. Maps
/// `SharedError::Cycle` to `-9`, `CapacityExceeded` to `-4`, `Type`
/// (null/unstorable) to `-3`.
///
/// # Safety
/// Key tuple per `decode_map_key`; `value_buf` valid for `vlen` bytes
/// (or `vlen == 0`).
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn oxphp_shared_map_set(
    entry_ptr: *const Entry,
    key_kind: c_int,
    key_int: i64,
    key_ptr: *const u8,
    key_len: usize,
    value_buf: *const u8,
    vlen: usize,
) -> c_int {
    if vlen > 0 && value_buf.is_null() {
        set_last_error("value_buf null with non-zero length");
        return SharedError::Generic.code();
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "map_set on freed Entry");
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { decode_map_key(key_kind, key_int, key_ptr, key_len)? };

        enforce_value_size(entry.registry, vlen)?;
        let value_bytes = if vlen == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(value_buf, vlen) }
        };
        let value = decode_value(value_bytes, entry.registry)?;

        let _prev = map.set(k, value)?;
        entry.registry.record_op(entry);
        Ok(())
    })
}

/// Atomic insert-if-absent. On success writes the prior value to
/// `out_prev_buf`/`out_prev_len` (callee frees) and `*out_absent = 1`
/// when the key was absent (i.e. the insert happened; no prev buffer).
///
/// # Safety
/// Key tuple per `decode_map_key`; value valid for `vlen`. The three
/// out pointers must be valid for writes.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn oxphp_shared_map_set_if_absent(
    entry_ptr: *const Entry,
    key_kind: c_int,
    key_int: i64,
    key_ptr: *const u8,
    key_len: usize,
    value_buf: *const u8,
    vlen: usize,
    out_prev_buf: *mut *mut u8,
    out_prev_len: *mut usize,
    out_absent: *mut c_int,
) -> c_int {
    if out_prev_buf.is_null() || out_prev_len.is_null() || out_absent.is_null() {
        set_last_error("null out pointer");
        return SharedError::Generic.code();
    }
    if vlen > 0 && value_buf.is_null() {
        set_last_error("value_buf null with non-zero length");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_prev_buf = std::ptr::null_mut();
        *out_prev_len = 0;
        *out_absent = 0;
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "map_set_if_absent on freed Entry");
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { decode_map_key(key_kind, key_int, key_ptr, key_len)? };
        enforce_value_size(entry.registry, vlen)?;
        let value_bytes = if vlen == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(value_buf, vlen) }
        };
        let value = decode_value(value_bytes, entry.registry)?;
        match map.set_if_absent(k, value)? {
            Some(prev) => {
                let bytes = sv_to_portbuf(&prev);
                drop(prev);
                let (ptr, n) = unsafe { payload_to_malloc(bytes)? };
                unsafe {
                    *out_prev_buf = ptr;
                    *out_prev_len = n;
                }
            }
            None => {
                unsafe { *out_absent = 1 };
            }
        }
        entry.registry.record_op(entry);
        Ok(())
    })
}

/// Overwrite and return the previous value. Writes the prior value to
/// `out_prev_buf`/`out_prev_len` (callee frees) or sets `*out_absent = 1`
/// when the key was absent.
///
/// # Safety
/// Key tuple per `decode_map_key`; value valid for `vlen`. Out pointers
/// valid for writes.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn oxphp_shared_map_swap(
    entry_ptr: *const Entry,
    key_kind: c_int,
    key_int: i64,
    key_ptr: *const u8,
    key_len: usize,
    value_buf: *const u8,
    vlen: usize,
    out_prev_buf: *mut *mut u8,
    out_prev_len: *mut usize,
    out_absent: *mut c_int,
) -> c_int {
    if out_prev_buf.is_null() || out_prev_len.is_null() || out_absent.is_null() {
        set_last_error("null out pointer");
        return SharedError::Generic.code();
    }
    if vlen > 0 && value_buf.is_null() {
        set_last_error("value_buf null with non-zero length");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_prev_buf = std::ptr::null_mut();
        *out_prev_len = 0;
        *out_absent = 0;
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "map_swap on freed Entry");
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { decode_map_key(key_kind, key_int, key_ptr, key_len)? };
        enforce_value_size(entry.registry, vlen)?;
        let value_bytes = if vlen == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(value_buf, vlen) }
        };
        let value = decode_value(value_bytes, entry.registry)?;
        match map.swap(k, value)? {
            Some(prev) => {
                let bytes = sv_to_portbuf(&prev);
                drop(prev);
                let (ptr, n) = unsafe { payload_to_malloc(bytes)? };
                unsafe {
                    *out_prev_buf = ptr;
                    *out_prev_len = n;
                }
            }
            None => {
                unsafe { *out_absent = 1 };
            }
        }
        entry.registry.record_op(entry);
        Ok(())
    })
}

/// Remove and return the previous value. Writes the prior value to
/// `out_prev_buf`/`out_prev_len` (callee frees) or sets `*out_absent = 1`
/// when the key was absent.
///
/// # Safety
/// Key tuple per `decode_map_key`. Out pointers valid for writes.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_pop(
    entry_ptr: *const Entry,
    key_kind: c_int,
    key_int: i64,
    key_ptr: *const u8,
    key_len: usize,
    out_prev_buf: *mut *mut u8,
    out_prev_len: *mut usize,
    out_absent: *mut c_int,
) -> c_int {
    if out_prev_buf.is_null() || out_prev_len.is_null() || out_absent.is_null() {
        set_last_error("null out pointer");
        return SharedError::Generic.code();
    }
    unsafe {
        *out_prev_buf = std::ptr::null_mut();
        *out_prev_len = 0;
        *out_absent = 0;
    }
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "map_pop on freed Entry");
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { decode_map_key(key_kind, key_int, key_ptr, key_len)? };
        match map.pop(&k) {
            Some(prev) => {
                let bytes = sv_to_portbuf(&prev);
                drop(prev);
                let (ptr, n) = unsafe { payload_to_malloc(bytes)? };
                unsafe {
                    *out_prev_buf = ptr;
                    *out_prev_len = n;
                }
            }
            None => {
                unsafe { *out_absent = 1 };
            }
        }
        entry.registry.record_op(entry);
        Ok(())
    })
}

/// Remove a key, discarding the value. `*out_existed` is `1` when the
/// key was present, `0` otherwise. No value buffer.
///
/// # Safety
/// Key tuple per `decode_map_key`. `out_existed` valid for a write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_remove(
    entry_ptr: *const Entry,
    key_kind: c_int,
    key_int: i64,
    key_ptr: *const u8,
    key_len: usize,
    out_existed: *mut c_int,
) -> c_int {
    if out_existed.is_null() {
        set_last_error("out_existed is null");
        return SharedError::Generic.code();
    }
    unsafe { *out_existed = 0 };
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "map_remove on freed Entry");
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { decode_map_key(key_kind, key_int, key_ptr, key_len)? };
        let existed = map.remove(&k);
        entry.registry.record_op(entry);
        unsafe { *out_existed = existed as c_int };
        Ok(())
    })
}

/// Atomic compare-and-set. `expected_is_absent` / `new_is_absent` mark
/// the corresponding side as the absence sentinel (PHP `null`); when
/// set, the matching `*_buf`/`*_len` are ignored. `*out_swapped` becomes
/// `1` iff the swap was applied.
///
/// # Safety
/// Key tuple per `decode_map_key`. `expected_buf` valid for `expected_len`
/// (when not absent); `new_buf` valid for `new_len` (when not absent).
/// `out_swapped` valid for a write.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn oxphp_shared_map_compare_and_set(
    entry_ptr: *const Entry,
    key_kind: c_int,
    key_int: i64,
    key_ptr: *const u8,
    key_len: usize,
    expected_buf: *const u8,
    expected_len: usize,
    expected_is_absent: c_int,
    new_buf: *const u8,
    new_len: usize,
    new_is_absent: c_int,
    out_swapped: *mut c_int,
) -> c_int {
    if out_swapped.is_null() {
        set_last_error("out_swapped is null");
        return SharedError::Generic.code();
    }
    unsafe { *out_swapped = 0 };
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(
            entry.magic, ENTRY_MAGIC,
            "map_compare_and_set on freed Entry"
        );
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;
        let k = unsafe { decode_map_key(key_kind, key_int, key_ptr, key_len)? };

        let expected = if expected_is_absent != 0 {
            None
        } else {
            let bytes = if expected_len == 0 {
                &[][..]
            } else {
                unsafe { std::slice::from_raw_parts(expected_buf, expected_len) }
            };
            Some(decode_value(bytes, entry.registry)?)
        };
        let new = if new_is_absent != 0 {
            None
        } else {
            enforce_value_size(entry.registry, new_len)?;
            let bytes = if new_len == 0 {
                &[][..]
            } else {
                unsafe { std::slice::from_raw_parts(new_buf, new_len) }
            };
            Some(decode_value(bytes, entry.registry)?)
        };

        let swapped = map.compare_and_set(k, expected, new)?;
        entry.registry.record_op(entry);
        unsafe { *out_swapped = swapped as c_int };
        Ok(())
    })
}

/// Bulk insert. `kv_buf` is a portbuf array of `[key, value]` 2-tuples
/// (the shape produced by `encode_pairs_portbuf`): each outer element is
/// a 2-element array whose `int_keyed[0]` is the key scalar (`Long` →
/// `MapKey::Int`, `String` → `MapKey::Str`) and `int_keyed[1]` is the
/// value. Per-key atomic; bail at first error with `*out_inserted`
/// holding the successful count.
///
/// # Safety
/// `kv_buf` valid for `kv_len` (or `kv_len == 0`). `out_inserted` valid
/// for a `u64` write.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_set_many(
    entry_ptr: *const Entry,
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
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        use crate::plugins::ox_shared::value::SharedValueRaw as R;
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "map_set_many on freed Entry");
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;

        let kv_bytes = if kv_len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(kv_buf, kv_len) }
        };
        let raw = portbuf_to_sv(kv_bytes)?;
        let arr = match raw {
            R::Array(a) => a,
            _ => {
                set_last_error("setMany expects an array of [key, value] pairs");
                return Err(SharedError::Type);
            }
        };
        let arr = Arc::try_unwrap(arr).unwrap_or_else(|shared| (*shared).clone());

        // Each outer element is a [key, value] 2-tuple (int_keyed half).
        let mut batch: Vec<(MapKey, SharedValue)> = Vec::with_capacity(arr.int_keyed.len());
        for pair_raw in arr.int_keyed.into_iter() {
            let pair = match pair_raw {
                R::Array(p) => Arc::try_unwrap(p).unwrap_or_else(|s| (*s).clone()),
                _ => {
                    set_last_error("setMany pair must be a [key, value] array");
                    return Err(SharedError::Type);
                }
            };
            let mut it = pair.int_keyed.into_iter();
            let key_raw = it.next().ok_or_else(|| {
                set_last_error("setMany pair missing key");
                SharedError::Type
            })?;
            let val_raw = it.next().ok_or_else(|| {
                set_last_error("setMany pair missing value");
                SharedError::Type
            })?;
            let key = raw_key_value_to_map_key(&key_raw)?;
            let value = raw_to_owned(val_raw, entry.registry)?;
            enforce_value_size(entry.registry, sv_to_portbuf(&value).len())?;
            batch.push((key, value));
        }

        match map.set_many_batch(batch) {
            Ok(n) => {
                unsafe { *out_inserted = n as u64 };
            }
            Err((e, n)) => {
                unsafe { *out_inserted = n as u64 };
                entry.registry.record_op(entry);
                return Err(e);
            }
        }
        entry.registry.record_op(entry);
        Ok(())
    })
}

/// Bulk remove. `keys_buf` is a portbuf `SharedValue::Array`; each
/// element's value is a key (long → `Int`, string → `Str`). Writes the
/// number actually removed to `*out_removed`.
///
/// # Safety
/// Per standard conventions for out pointers + keys buffer.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_remove_many(
    entry_ptr: *const Entry,
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
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "map_remove_many on freed Entry");
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;

        let keys_bytes = if keys_len == 0 {
            &[][..]
        } else {
            unsafe { std::slice::from_raw_parts(keys_buf, keys_len) }
        };
        let raw = portbuf_to_sv(keys_bytes)?;
        let arr = match raw {
            crate::plugins::ox_shared::value::SharedValueRaw::Array(a) => a,
            _ => {
                set_last_error("removeMany expects an array of keys");
                return Err(SharedError::Type);
            }
        };

        let mut removed: u64 = 0;
        for key_sv in &arr.int_keyed {
            let k = raw_key_value_to_map_key(key_sv)?;
            if map.remove(&k) {
                removed += 1;
            }
        }
        entry.registry.record_op(entry);
        unsafe { *out_removed = removed };
        Ok(())
    })
}

/// Whole-map key snapshot in a single `entries.iter()` pass, serialised
/// as a portbuf `SharedValue::Array`: every key lands in the int-keyed
/// half (`Int` → `Long`, `Str` → `String`); the PHP decoder distinguishes
/// by zval type. `forEach` calls this once (O(n)) and then re-fetches each
/// value, rather than partitioning the map per stripe (which degenerated
/// to a full scan per stripe because the logical stripes don't line up
/// with DashMap's physical shards).
///
/// # Safety
/// `out_buf`, `out_len` must be valid for writes.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_map_all_keys(
    entry_ptr: *const Entry,
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
    if entry_ptr.is_null() {
        return SharedError::StaleHandle.code();
    }
    ffi_entry(|| {
        let entry: &Entry = unsafe { &*entry_ptr };
        debug_assert_eq!(entry.magic, ENTRY_MAGIC, "map_all_keys on freed Entry");
        let map = entry.inner.as_any_map().ok_or(SharedError::Type)?;

        let mut arr = crate::plugins::ox_shared::value::SharedArray::default();
        for k in map.all_keys() {
            arr.int_keyed.push(map_key_to_shared_value(&k));
        }
        entry.registry.record_op(entry);

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

/// Encode a `MapKey` as a `SharedValue` for shard-key serialisation:
/// `Int` → `Long`, `Str` → `String`.
fn map_key_to_shared_value(k: &MapKey) -> SharedValue {
    match k {
        MapKey::Int(i) => SharedValue::Long(*i),
        // Keys are opaque bytes — surface them as a binary-safe value.
        MapKey::Str(s) => SharedValue::Bytes(Arc::clone(s)),
    }
}

/// Decode a raw key value (from `removeMany`'s key array) into a
/// `MapKey`: `Long` → `Int`, `String`/`Bytes` → `Str`.
fn raw_key_value_to_map_key(
    raw: &crate::plugins::ox_shared::value::SharedValueRaw,
) -> Result<MapKey, SharedError> {
    use crate::plugins::ox_shared::value::SharedValueRaw as R;
    match raw {
        R::Long(i) => Ok(MapKey::Int(*i)),
        // Both string and byte values become binary-safe opaque-byte keys.
        R::String(s) => Ok(MapKey::Str(Arc::from(s.as_bytes()))),
        R::Bytes(b) => Ok(MapKey::Str(Arc::clone(b))),
        _ => {
            set_last_error("map keys must be int or string");
            Err(SharedError::Type)
        }
    }
}

/// Downcast helper mirroring `SharedInnerCounterExt` — used by the FFI
/// functions above.
pub trait SharedInnerMapExt {
    fn as_any_map(&self) -> Option<&MapInner>;
}

impl SharedInnerMapExt for dyn SharedInner {
    fn as_any_map(&self) -> Option<&MapInner> {
        self.as_any().downcast_ref::<MapInner>()
    }
}

// ─── PHP class registration ───────────────────────────────────────────────

use crate::bridge::ffi as bridge_ffi;
use crate::plugin::types::{MagicMethod, PhpType, PhpValue};
use crate::plugin::{PhpError, PluginContext, PluginError};
use crate::plugins::ox_shared::handle::SharedHandle;

/// FQN of the lazy `getMany` iterator class.
const KEY_CURSOR_FQN: &str = "OxPHP\\Shared\\Map\\KeyCursor";

/// Map `SharedError` FFI codes onto the `Shared\*` exception hierarchy.
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
        -11 => "OxPHP\\Shared\\ValueTooLargeException",
        _ => "OxPHP\\Shared\\SharedException",
    };
    Err(PhpError::Exception {
        class: class.to_string(),
        message: read_last_error_message(),
        code: 0,
    })
}

/// Decoded PHP key argument: the tagged tuple passed to FFI key fns.
struct KeyArg {
    kind: c_int,
    int: i64,
    /// Backing storage for a string key (kept alive while the FFI call
    /// borrows `ptr`/`len`).
    bytes: Vec<u8>,
}

impl KeyArg {
    fn ptr(&self) -> *const u8 {
        self.bytes.as_ptr()
    }
    fn len(&self) -> usize {
        self.bytes.len()
    }
}

/// Decode the PHP key zval at arg `idx`: `IS_LONG` → Int, `IS_STRING`
/// → Str, anything else → TypeException.
fn decode_key_arg(call: &crate::bridge::call::NativeCall, idx: u32) -> Result<KeyArg, PhpError> {
    use crate::bridge::types::ValType;
    match call.arg_type(idx)? {
        ValType::Long => Ok(KeyArg {
            kind: KEY_KIND_INT,
            int: call.arg_long(idx)?,
            bytes: Vec::new(),
        }),
        ValType::String => {
            let s = call.arg_bytes(idx)?;
            Ok(KeyArg {
                kind: KEY_KIND_STR,
                int: 0,
                bytes: s.to_vec(),
            })
        }
        other => Err(PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".to_string(),
            message: format!(
                "Shared\\Map key must be int or string, got {}",
                crate::bridge::call::type_name(other)
            ),
            code: 0,
        }),
    }
}

/// Serialise arg `idx` (a `mixed` PHP value) to a libc-malloc'd portbuf
/// buffer. Caller frees with `oxphp_portable_free`. Returns
/// `TypeException` on any non-serialisable value.
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

/// Deserialise `(buf, len)` portbuf into `call`'s return-value zval.
/// Always frees `buf`. On decode failure, sets return to null.
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

/// Reject a `null` value arg before serialising (the null-as-absence
/// invariant). Returns `Ok(())` if the value is non-null.
fn reject_null_value(
    call: &crate::bridge::call::NativeCall,
    idx: u32,
    label: &str,
) -> Result<(), PhpError> {
    if call.arg_is_null(idx).unwrap_or(false) {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".to_string(),
            message: format!(
                "Shared\\Map::{label}: null is not a storable value \
                 (null means absence; use remove() instead)"
            ),
            code: 0,
        });
    }
    Ok(())
}

pub fn register_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    register_key_cursor_class(ctx)?;

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

            let mut out_ptr: *const Entry = std::ptr::null();
            let rc = unsafe { oxphp_shared_map_create(max_entries, &mut out_ptr) };
            map_rc_to_result(rc)?;

            let h = call.storage_mut::<SharedHandle>()?;
            h.entry_ptr = out_ptr;
            h.type_tag = SharedType::Map as u8;
            Ok(())
        })
        // ── get(int|string $key): mixed ────────────────────────────────
        .method("get")
        .param("key", PhpType::Union(vec![PhpType::Int, PhpType::String]))
        .returns(PhpType::Mixed)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let key = decode_key_arg(call, 0)?;
            let mut buf: *mut u8 = std::ptr::null_mut();
            let mut len: usize = 0;
            let mut absent: c_int = 0;
            let rc = unsafe {
                oxphp_shared_map_get(
                    entry_ptr,
                    key.kind,
                    key.int,
                    key.ptr(),
                    key.len(),
                    &mut buf,
                    &mut len,
                    &mut absent,
                )
            };
            map_rc_to_result(rc)?;
            if absent != 0 {
                call.ret_null();
                return Ok(());
            }
            deserialize_into_retval(call, buf, len);
            Ok(())
        })
        // ── set(int|string $key, mixed $value): void ───────────────────
        .method("set")
        .param("key", PhpType::Union(vec![PhpType::Int, PhpType::String]))
        .param("value", PhpType::Mixed)
        .returns(PhpType::Void)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let key = decode_key_arg(call, 0)?;
            reject_null_value(call, 1, "set")?;
            let (vbuf, vlen) = serialize_mixed_arg(call, 1, "set")?;
            let rc = unsafe {
                oxphp_shared_map_set(
                    entry_ptr,
                    key.kind,
                    key.int,
                    key.ptr(),
                    key.len(),
                    vbuf,
                    vlen,
                )
            };
            unsafe { bridge_ffi::oxphp_portable_free(vbuf) };
            map_rc_to_result(rc)?;
            call.ret_null();
            Ok(())
        })
        // ── setIfAbsent(int|string $key, mixed $value): mixed ──────────
        .method("setIfAbsent")
        .param("key", PhpType::Union(vec![PhpType::Int, PhpType::String]))
        .param("value", PhpType::Mixed)
        .returns(PhpType::Mixed)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let key = decode_key_arg(call, 0)?;
            reject_null_value(call, 1, "setIfAbsent")?;
            let (vbuf, vlen) = serialize_mixed_arg(call, 1, "setIfAbsent")?;
            let mut prev_buf: *mut u8 = std::ptr::null_mut();
            let mut prev_len: usize = 0;
            let mut absent: c_int = 0;
            let rc = unsafe {
                oxphp_shared_map_set_if_absent(
                    entry_ptr,
                    key.kind,
                    key.int,
                    key.ptr(),
                    key.len(),
                    vbuf,
                    vlen,
                    &mut prev_buf,
                    &mut prev_len,
                    &mut absent,
                )
            };
            unsafe { bridge_ffi::oxphp_portable_free(vbuf) };
            map_rc_to_result(rc)?;
            if absent != 0 {
                // Inserted — return null.
                call.ret_null();
                return Ok(());
            }
            deserialize_into_retval(call, prev_buf, prev_len);
            Ok(())
        })
        // ── swap(int|string $key, mixed $value): mixed ─────────────────
        .method("swap")
        .param("key", PhpType::Union(vec![PhpType::Int, PhpType::String]))
        .param("value", PhpType::Mixed)
        .returns(PhpType::Mixed)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let key = decode_key_arg(call, 0)?;
            reject_null_value(call, 1, "swap")?;
            let (vbuf, vlen) = serialize_mixed_arg(call, 1, "swap")?;
            let mut prev_buf: *mut u8 = std::ptr::null_mut();
            let mut prev_len: usize = 0;
            let mut absent: c_int = 0;
            let rc = unsafe {
                oxphp_shared_map_swap(
                    entry_ptr,
                    key.kind,
                    key.int,
                    key.ptr(),
                    key.len(),
                    vbuf,
                    vlen,
                    &mut prev_buf,
                    &mut prev_len,
                    &mut absent,
                )
            };
            unsafe { bridge_ffi::oxphp_portable_free(vbuf) };
            map_rc_to_result(rc)?;
            if absent != 0 {
                call.ret_null();
                return Ok(());
            }
            deserialize_into_retval(call, prev_buf, prev_len);
            Ok(())
        })
        // ── pop(int|string $key): mixed ────────────────────────────────
        .method("pop")
        .param("key", PhpType::Union(vec![PhpType::Int, PhpType::String]))
        .returns(PhpType::Mixed)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let key = decode_key_arg(call, 0)?;
            let mut prev_buf: *mut u8 = std::ptr::null_mut();
            let mut prev_len: usize = 0;
            let mut absent: c_int = 0;
            let rc = unsafe {
                oxphp_shared_map_pop(
                    entry_ptr,
                    key.kind,
                    key.int,
                    key.ptr(),
                    key.len(),
                    &mut prev_buf,
                    &mut prev_len,
                    &mut absent,
                )
            };
            map_rc_to_result(rc)?;
            if absent != 0 {
                call.ret_null();
                return Ok(());
            }
            deserialize_into_retval(call, prev_buf, prev_len);
            Ok(())
        })
        // ── remove(int|string $key): bool ──────────────────────────────
        .method("remove")
        .param("key", PhpType::Union(vec![PhpType::Int, PhpType::String]))
        .returns(PhpType::Bool)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let key = decode_key_arg(call, 0)?;
            let mut existed: c_int = 0;
            let rc = unsafe {
                oxphp_shared_map_remove(
                    entry_ptr,
                    key.kind,
                    key.int,
                    key.ptr(),
                    key.len(),
                    &mut existed,
                )
            };
            map_rc_to_result(rc)?;
            call.ret_bool(existed != 0);
            Ok(())
        })
        // ── compareAndSet(int|string $key, mixed $expected, mixed $new): bool
        .method("compareAndSet")
        .param("key", PhpType::Union(vec![PhpType::Int, PhpType::String]))
        .param("expected", PhpType::Mixed)
        .param("new", PhpType::Mixed)
        .returns(PhpType::Bool)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let key = decode_key_arg(call, 0)?;

            let expected_absent = call.arg_is_null(1).unwrap_or(true);
            let new_absent = call.arg_is_null(2).unwrap_or(true);

            let (exp_buf, exp_len) = if expected_absent {
                (std::ptr::null_mut(), 0usize)
            } else {
                serialize_mixed_arg(call, 1, "compareAndSet")?
            };
            let (new_buf, new_len) = if new_absent {
                (std::ptr::null_mut(), 0usize)
            } else {
                match serialize_mixed_arg(call, 2, "compareAndSet") {
                    Ok(v) => v,
                    Err(e) => {
                        if !exp_buf.is_null() {
                            unsafe { bridge_ffi::oxphp_portable_free(exp_buf) };
                        }
                        return Err(e);
                    }
                }
            };

            let mut swapped: c_int = 0;
            let rc = unsafe {
                oxphp_shared_map_compare_and_set(
                    entry_ptr,
                    key.kind,
                    key.int,
                    key.ptr(),
                    key.len(),
                    exp_buf,
                    exp_len,
                    expected_absent as c_int,
                    new_buf,
                    new_len,
                    new_absent as c_int,
                    &mut swapped,
                )
            };
            if !exp_buf.is_null() {
                unsafe { bridge_ffi::oxphp_portable_free(exp_buf) };
            }
            if !new_buf.is_null() {
                unsafe { bridge_ffi::oxphp_portable_free(new_buf) };
            }
            map_rc_to_result(rc)?;
            call.ret_bool(swapped != 0);
            Ok(())
        })
        // ── setMany(iterable $entries): int ────────────────────────────
        .method("setMany")
        .param("entries", PhpType::Iterable)
        .returns(PhpType::Int)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            // Materialise the iterable's key=>value pairs into a portbuf
            // SharedValue::Array, then hand to the batched FFI.
            let pairs = collect_iterable_pairs(call, 0)?;
            let bytes = encode_pairs_portbuf(&pairs);
            let mut inserted: u64 = 0;
            let rc = unsafe {
                oxphp_shared_map_set_many(entry_ptr, bytes.as_ptr(), bytes.len(), &mut inserted)
            };
            map_rc_to_result(rc)?;
            call.ret_long(inserted as i64);
            Ok(())
        })
        // ── removeMany(iterable $keys): int ────────────────────────────
        .method("removeMany")
        .param("keys", PhpType::Iterable)
        .returns(PhpType::Int)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let keys = collect_iterable_keys(call, 0)?;
            let bytes = encode_keys_portbuf(&keys);
            let mut removed: u64 = 0;
            let rc = unsafe {
                oxphp_shared_map_remove_many(entry_ptr, bytes.as_ptr(), bytes.len(), &mut removed)
            };
            map_rc_to_result(rc)?;
            call.ret_long(removed as i64);
            Ok(())
        })
        // ── getMany(iterable $keys): \Iterator ─────────────────────────
        .method("getMany")
        .param("keys", PhpType::Iterable)
        .returns(PhpType::Class("Iterator".to_string()))
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            construct_key_cursor(call, entry_ptr, 0)
        })
        // ── clear(): int ───────────────────────────────────────────────
        .method("clear")
        .returns(PhpType::Int)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let mut removed: u64 = 0;
            let rc = unsafe { oxphp_shared_map_clear(entry_ptr, &mut removed) };
            map_rc_to_result(rc)?;
            call.ret_long(removed as i64);
            Ok(())
        })
        // ── count(): int ───────────────────────────────────────────────
        .method("count")
        .returns(PhpType::Int)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            let mut c: u64 = 0;
            let rc = unsafe { oxphp_shared_map_count(entry_ptr, &mut c) };
            map_rc_to_result(rc)?;
            call.ret_long(c as i64);
            Ok(())
        })
        // ── maxEntries(): ?int ─────────────────────────────────────────
        .method("maxEntries")
        .returns(PhpType::Nullable(Box::new(PhpType::Int)))
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            if entry_ptr.is_null() {
                return Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\StaleHandleException".to_string(),
                    message: "Map handle is no longer alive".to_string(),
                    code: 0,
                });
            }
            // SAFETY: entry_ptr is non-null and a live Arc::into_raw
            // pointer per the handle contract.
            let entry: &Entry = unsafe { &*entry_ptr };
            debug_assert_eq!(entry.magic, ENTRY_MAGIC, "maxEntries on freed Entry");
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
        // ── forEach(callable $fn): void ────────────────────────────────
        .method("forEach")
        .param("fn", PhpType::Callable)
        .returns(PhpType::Void)
        .handler(|call| {
            let entry_ptr = call.storage::<SharedHandle>()?.entry_ptr;
            for_each_impl(call, entry_ptr)?;
            call.ret_null();
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
            let id =
                unsafe { crate::plugins::ox_shared::registry::oxphp_shared_entry_id(h.entry_ptr) };
            call.ret_long(id as i64);
            Ok(())
        })
        .build()?;

    Ok(())
}

// ─── iterable marshalling helpers ──────────────────────────────────
//
// `setMany` / `removeMany` / `getMany` accept any PHP `iterable`. The
// argument may be an array (fast path: `arg_array_foreach`) or a
// Traversable (generic path). We materialise into Rust Vecs and re-encode
// into the portbuf shapes the FFI fns expect.

/// One materialised key=>value pair from an iterable.
struct IterPair {
    key: MapKey,
    value: Vec<u8>, // portbuf-serialised value
}

/// Collect key=>value pairs from an `iterable` argument. Arrays use the
/// zero-copy `arg_array_foreach`; other Traversables are materialised via
/// PHP `iterator_to_array($it, true)`.
fn collect_iterable_pairs(
    call: &mut crate::bridge::call::NativeCall,
    idx: u32,
) -> Result<Vec<IterPair>, PhpError> {
    use crate::bridge::call::ArrayKey;
    use crate::bridge::types::ValType;

    let mut out: Vec<IterPair> = Vec::new();
    let mut err: Option<PhpError> = None;

    // Push one materialised (key, value) pair, recording the first error.
    let mut collect = |key: MapKey, v: &crate::bridge::call::Val| {
        if err.is_some() {
            return;
        }
        match serialize_val(v) {
            Ok(bytes) => out.push(IterPair { key, value: bytes }),
            Err(e) => err = Some(e),
        }
    };

    if call.arg_type(idx)? == ValType::Array {
        // Array fast path: keys are binary-safe (raw bytes, no UTF-8 coercion).
        call.arg_array_foreach_raw(idx, |key_bytes, num_idx, v| {
            let key = match key_bytes {
                Some(b) => MapKey::Str(Arc::from(b)),
                None => MapKey::Int(num_idx),
            };
            collect(key, &v);
        })?;
    } else {
        // Non-array iterable: iterator_to_array($it, preserve_keys: true).
        // This path goes through `Val::foreach`, which yields `ArrayKey` —
        // a UTF-8-coerced string key. A non-UTF-8 Generator key therefore
        // still coerces here; the array path above is the faithful one.
        let src = unsafe { call.raw_arg_ptr(idx) };
        let result = call.call_php("iterator_to_array", 2, |b| {
            unsafe { b.zval_copy(src) };
            b.bool(true);
        })?;
        result.val().foreach(|k, v| {
            let key = match k {
                ArrayKey::Int(i) => MapKey::Int(i),
                ArrayKey::Str(s) => MapKey::Str(Arc::from(s.as_bytes())),
            };
            collect(key, &v);
        });
    }

    if let Some(e) = err {
        return Err(e);
    }
    Ok(out)
}

/// Collect keys from an `iterable` of int|string.
fn collect_iterable_keys(
    call: &mut crate::bridge::call::NativeCall,
    idx: u32,
) -> Result<Vec<MapKey>, PhpError> {
    use crate::bridge::call::Val;
    use crate::bridge::types::ValType;

    let mut out: Vec<MapKey> = Vec::new();
    let mut err: Option<PhpError> = None;

    let push_val = |out: &mut Vec<MapKey>, v: &Val| -> Result<(), PhpError> {
        match v.val_type() {
            ValType::Long => {
                out.push(MapKey::Int(v.as_long()));
                Ok(())
            }
            // String keys are binary-safe: store the raw bytes opaquely.
            ValType::String => match v.as_bytes() {
                Some(b) => {
                    out.push(MapKey::Str(Arc::from(b)));
                    Ok(())
                }
                None => Err(PhpError::Exception {
                    class: "OxPHP\\Shared\\TypeException".to_string(),
                    message: "Shared\\Map string keys could not be read".to_string(),
                    code: 0,
                }),
            },
            other => Err(PhpError::Exception {
                class: "OxPHP\\Shared\\TypeException".to_string(),
                message: format!(
                    "Shared\\Map keys must be int or string, got {}",
                    crate::bridge::call::type_name(other)
                ),
                code: 0,
            }),
        }
    };

    if call.arg_type(idx)? == ValType::Array {
        call.arg_array_foreach(idx, |_k, v| {
            if err.is_some() {
                return;
            }
            if let Err(e) = push_val(&mut out, &v) {
                err = Some(e);
            }
        })?;
        if let Some(e) = err {
            return Err(e);
        }
        return Ok(out);
    }

    // Non-array iterable.
    let src = unsafe { call.raw_arg_ptr(idx) };
    let result = call.call_php("iterator_to_array", 2, |b| {
        unsafe { b.zval_copy(src) };
        b.bool(false); // keys irrelevant for a key list
    })?;
    let val = result.val();
    val.foreach(|_k, v| {
        if err.is_some() {
            return;
        }
        if let Err(e) = push_val(&mut out, &v) {
            err = Some(e);
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    Ok(out)
}

/// Serialise a `Val` (array element / iterator value) to portbuf bytes
/// via the bridge serializer. Rejects non-serialisable values.
fn serialize_val(v: &crate::bridge::call::Val) -> Result<Vec<u8>, PhpError> {
    let mut buf: *mut u8 = std::ptr::null_mut();
    let mut len: usize = 0;
    let rc = unsafe {
        bridge_ffi::oxphp_portable_serialize(v.as_ptr() as *const _, 1, &mut buf, &mut len)
    };
    if rc != 0 {
        if !buf.is_null() {
            unsafe { bridge_ffi::oxphp_portable_free(buf) };
        }
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\TypeException".to_string(),
            message: "Shared\\Map: value is not serialisable (closure/resource)".to_string(),
            code: 0,
        });
    }
    let bytes = if buf.is_null() || len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(buf, len).to_vec() }
    };
    if !buf.is_null() {
        unsafe { bridge_ffi::oxphp_portable_free(buf) };
    }
    Ok(bytes)
}

/// Encode materialised `(MapKey, portbuf)` pairs into a single portbuf
/// `SharedValue::Array` for `setMany`. Int keys go to the int-keyed half
/// (preserving the index), string keys to the str-keyed half.
fn encode_pairs_portbuf(pairs: &[IterPair]) -> Vec<u8> {
    // The wire codec only carries int-keyed (dense, index-implicit) and
    // str-keyed entries. To preserve explicit int keys we route every
    // entry through the str-keyed half is wrong (loses int distinctness);
    // instead build a SharedValue::Array honouring both halves and rely
    // on the FFI decode mapping (int half → MapKey::Int(index)). Since
    // int indices in the wire format are positional, we cannot carry an
    // arbitrary int key through the int half. We therefore encode int
    // keys as a tagged scalar in the str half is also wrong. So: encode
    // the pairs directly with a small custom layout the FFI understands.
    //
    // Simpler & correct: serialise each (key, value) pair flat. We reuse
    // the portbuf array shape where the int-keyed half holds VALUES at
    // their positional index for dense int keys 0..n, and str-keyed holds
    // string-key entries. For non-dense int keys we fall back to the
    // str-keyed half with the decimal key string — but that would alias
    // "123" and 123. To keep int|string distinct, build a 2-level array:
    //   [ ['k'=>int_or_str_key, 'v'=>value], ... ]
    // and have set_many decode each pair. Implemented below.
    let mut buf = Vec::with_capacity(8 + pairs.len() * 16);
    buf.push(6u8); // array tag
    buf.extend_from_slice(&(pairs.len() as u32).to_le_bytes());
    for (i, p) in pairs.iter().enumerate() {
        buf.push(0u8); // index key type
        buf.extend_from_slice(&(i as u64).to_le_bytes());
        // value: a 2-entry array {0: key, 1: value}
        buf.push(6u8);
        buf.extend_from_slice(&2u32.to_le_bytes());
        // entry 0 (index 0): the key
        buf.push(0u8);
        buf.extend_from_slice(&0u64.to_le_bytes());
        match &p.key {
            MapKey::Int(n) => {
                buf.push(3u8);
                buf.extend_from_slice(&n.to_le_bytes());
            }
            MapKey::Str(s) => {
                buf.push(5u8);
                let kb = &s[..];
                buf.extend_from_slice(&(kb.len() as u32).to_le_bytes());
                buf.extend_from_slice(kb);
            }
        }
        // entry 1 (index 1): the already-serialised value (raw portbuf)
        buf.push(0u8);
        buf.extend_from_slice(&1u64.to_le_bytes());
        buf.extend_from_slice(&p.value);
    }
    buf
}

/// Encode keys into a portbuf `SharedValue::Array` (int-keyed half holds
/// the key scalars) for `removeMany`.
fn encode_keys_portbuf(keys: &[MapKey]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(8 + keys.len() * 12);
    buf.push(6u8); // array tag
    buf.extend_from_slice(&(keys.len() as u32).to_le_bytes());
    for (i, k) in keys.iter().enumerate() {
        buf.push(0u8); // index key type
        buf.extend_from_slice(&(i as u64).to_le_bytes());
        match k {
            MapKey::Int(n) => {
                buf.push(3u8);
                buf.extend_from_slice(&n.to_le_bytes());
            }
            MapKey::Str(s) => {
                buf.push(5u8);
                let kb = &s[..];
                buf.extend_from_slice(&(kb.len() as u32).to_le_bytes());
                buf.extend_from_slice(kb);
            }
        }
    }
    buf
}

// ─── forEach ───────────────────────────────────────────────────────

/// Snapshot all keys once via `oxphp_shared_map_all_keys`, then per key
/// fetch the value and invoke the PHP callback through
/// `oxphp_shared_invoke_2_ret_stop`. No shard lock is held across the PHP
/// call. A `false` return (stop flag) ends iteration early.
fn for_each_impl(
    call: &mut crate::bridge::call::NativeCall,
    entry_ptr: *const Entry,
) -> Result<(), PhpError> {
    let callable = unsafe { call.raw_arg_ptr(0) };

    // Snapshot every key in a single O(n) pass, then re-fetch each value
    // and invoke the callback with no map lock held. Keys deleted between
    // snapshot and re-fetch are skipped. Only keys are pinned (cheap), so a
    // slow callback never holds a value alive.
    let mut keys_buf: *mut u8 = std::ptr::null_mut();
    let mut keys_len: usize = 0;
    let rc = unsafe { oxphp_shared_map_all_keys(entry_ptr, &mut keys_buf, &mut keys_len) };
    map_rc_to_result(rc)?;
    let keys = decode_key_array(keys_buf, keys_len);
    if !keys_buf.is_null() {
        unsafe { bridge_ffi::oxphp_portable_free(keys_buf) };
    }

    for key in keys {
        // Fetch the current value (skip if now absent).
        let (kind, kint, kbytes) = key_to_ffi(&key);
        let mut vbuf: *mut u8 = std::ptr::null_mut();
        let mut vlen: usize = 0;
        let mut absent: c_int = 0;
        let rc = unsafe {
            oxphp_shared_map_get(
                entry_ptr,
                kind,
                kint,
                kbytes.as_ptr(),
                kbytes.len(),
                &mut vbuf,
                &mut vlen,
                &mut absent,
            )
        };
        map_rc_to_result(rc)?;
        if absent != 0 {
            continue; // deleted between snapshot and re-fetch
        }

        let stop = unsafe {
            bridge_ffi::oxphp_shared_invoke_2_ret_stop(
                callable,
                kind,
                kint,
                kbytes.as_ptr(),
                kbytes.len(),
                vbuf,
                vlen,
            )
        };
        if !vbuf.is_null() {
            unsafe { bridge_ffi::oxphp_portable_free(vbuf) };
        }
        if stop < 0 {
            // Callback threw or invalid callable — EG(exception) is
            // already set on the PHP side; bail without overwriting.
            return Err(PhpError::Custom("Map::forEach callback failed".into()));
        }
        if stop != 0 {
            return Ok(()); // callback returned false → stop early
        }
    }
    Ok(())
}

/// FFI key tuple for a `MapKey`: (kind, int, bytes).
fn key_to_ffi(key: &MapKey) -> (c_int, i64, Vec<u8>) {
    match key {
        MapKey::Int(i) => (KEY_KIND_INT, *i, Vec::new()),
        MapKey::Str(s) => (KEY_KIND_STR, 0, s.to_vec()),
    }
}

/// Decode a key-snapshot portbuf array (int-keyed half holds Long/String
/// key scalars) into a `Vec<MapKey>`.
fn decode_key_array(buf: *mut u8, len: usize) -> Vec<MapKey> {
    if buf.is_null() || len == 0 {
        return Vec::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(buf, len) };
    let raw = match portbuf_to_sv(bytes) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };
    let arr = match raw {
        crate::plugins::ox_shared::value::SharedValueRaw::Array(a) => a,
        _ => return Vec::new(),
    };
    arr.int_keyed
        .iter()
        .filter_map(|v| raw_key_value_to_map_key(v).ok())
        .collect()
}

// ─── Map\KeyCursor ─────────────────────────────────────────────────
//
// A native lazy Iterator returned by `getMany`. It pins the Map (one
// strong Arc) and a materialised list of keys captured at construction;
// each `current()` re-fetches one value via the single-key getter,
// skipping keys now absent. No Map lock is held across iteration.

/// Per-instance Rust storage for a `Map\KeyCursor`.
struct KeyCursorState {
    /// Strong reference to the Map entry (Arc bump). Reconstituted and
    /// dropped on cursor Drop.
    entry_ptr: *const Entry,
    /// Materialised key list captured from the `iterable` at construction.
    keys: Vec<MapKey>,
    /// Cursor position into `keys`.
    index: usize,
    /// Cached portbuf bytes of the value at the current valid position
    /// (None until positioned / when invalid).
    current: Option<Vec<u8>>,
}

// SAFETY: `entry_ptr` is an `Arc::into_raw(Arc<Entry>)`; `Entry: Send +
// Sync`. The cursor owns one strong ref, so moving across threads is
// sound (same as moving an Arc<Entry>).
unsafe impl Send for KeyCursorState {}
unsafe impl Sync for KeyCursorState {}

impl Drop for KeyCursorState {
    fn drop(&mut self) {
        if !self.entry_ptr.is_null() {
            // SAFETY: entry_ptr came from Arc::into_raw via
            // construct_key_cursor; reconstitute and drop exactly once.
            unsafe { drop(Arc::from_raw(self.entry_ptr)) };
        }
    }
}

/// Storage wrapper: a single raw pointer to a boxed [`KeyCursorState`].
/// `#[repr(C)]` so the C-side allocator helper can `memcpy` the pointer
/// into the rust_data slot (mirrors `PoolHandleStorage`). NULL = not yet
/// populated (factory default).
#[repr(C)]
struct KeyCursorStorage {
    state: *mut KeyCursorState,
}

// SAFETY: the pointed-to KeyCursorState is Send + Sync; the wrapper just
// carries a raw pointer with single ownership.
unsafe impl Send for KeyCursorStorage {}
unsafe impl Sync for KeyCursorStorage {}

impl Default for KeyCursorStorage {
    fn default() -> Self {
        Self {
            state: std::ptr::null_mut(),
        }
    }
}

impl Drop for KeyCursorStorage {
    fn drop(&mut self) {
        if !self.state.is_null() {
            // SAFETY: state was Box::into_raw'd in construct_key_cursor.
            unsafe { drop(Box::from_raw(self.state)) };
            self.state = std::ptr::null_mut();
        }
    }
}

/// Build a `Map\KeyCursor` into the retval. Materialises the iterable's
/// keys, bumps the Map's Arc, boxes the state, and hands the box pointer
/// to the C-side allocator which constructs the object and stamps the
/// pointer into its rust_data.
fn construct_key_cursor(
    call: &mut crate::bridge::call::NativeCall,
    entry_ptr: *const Entry,
    keys_idx: u32,
) -> Result<(), PhpError> {
    if entry_ptr.is_null() {
        return Err(PhpError::Exception {
            class: "OxPHP\\Shared\\StaleHandleException".to_string(),
            message: "Map handle is no longer alive".to_string(),
            code: 0,
        });
    }
    let keys = collect_iterable_keys(call, keys_idx)?;

    // Bump the Map's strong refcount so the cursor keeps the entry alive.
    // SAFETY: entry_ptr is a live Arc::into_raw pointer (the Map wrapper
    // holds a strong ref for the duration of this call).
    unsafe { Arc::increment_strong_count(entry_ptr) };

    let state = Box::new(KeyCursorState {
        entry_ptr,
        keys,
        index: 0,
        current: None,
    });
    let state_ptr = Box::into_raw(state);

    let retval = call.retval_ptr();
    let rc = unsafe {
        bridge_ffi::oxphp_shared_map_cursor_alloc(retval, state_ptr as *mut std::os::raw::c_void)
    };
    if rc != 0 {
        // Allocation failed — reclaim the box (its Drop releases the Arc).
        // SAFETY: state_ptr was just Box::into_raw'd and not consumed.
        unsafe { drop(Box::from_raw(state_ptr)) };
        return Err(PhpError::Custom(
            "failed to construct Map\\KeyCursor".into(),
        ));
    }
    Ok(())
}

/// Read the boxed cursor-state pointer from `$this`'s rust_data
/// storage. Returns a raw pointer (Copy) so callers can build a `&mut`
/// to the pointee independent of `call`'s borrow — the cursor is
/// single-threaded per request, so aliasing is not a concern.
fn cursor_state_ptr(
    call: &crate::bridge::call::NativeCall,
) -> Result<*mut KeyCursorState, PhpError> {
    let storage = call.storage::<KeyCursorStorage>()?;
    if storage.state.is_null() {
        return Err(PhpError::Custom("KeyCursor not initialised".into()));
    }
    Ok(storage.state)
}

/// Re-fetch the value at the cursor's current key, advancing past keys
/// that are now absent. Updates `state.current`. Stops at the first
/// present key or at the end of the key list.
fn cursor_advance_to_present(state: &mut KeyCursorState) {
    state.current = None;
    while state.index < state.keys.len() {
        let key = state.keys[state.index].clone();
        let (kind, kint, kbytes) = key_to_ffi(&key);
        let mut buf: *mut u8 = std::ptr::null_mut();
        let mut len: usize = 0;
        let mut absent: c_int = 0;
        let rc = unsafe {
            oxphp_shared_map_get(
                state.entry_ptr,
                kind,
                kint,
                kbytes.as_ptr(),
                kbytes.len(),
                &mut buf,
                &mut len,
                &mut absent,
            )
        };
        if rc != 0 {
            // Stale handle / error: treat as end of iteration.
            state.index = state.keys.len();
            return;
        }
        if absent != 0 {
            if !buf.is_null() {
                unsafe { bridge_ffi::oxphp_portable_free(buf) };
            }
            state.index += 1;
            continue;
        }
        let bytes = if buf.is_null() || len == 0 {
            Vec::new()
        } else {
            unsafe { std::slice::from_raw_parts(buf, len).to_vec() }
        };
        if !buf.is_null() {
            unsafe { bridge_ffi::oxphp_portable_free(buf) };
        }
        state.current = Some(bytes);
        return;
    }
}

fn register_key_cursor_class(ctx: &mut PluginContext) -> Result<(), PluginError> {
    ctx.register_class(KEY_CURSOR_FQN)
        .final_()
        .implements("Iterator")
        .with_storage(KeyCursorStorage::default)
        // ── rewind(): void ─────────────────────────────────────────
        .method("rewind")
        .returns(PhpType::Void)
        .handler(|call| {
            let sp = cursor_state_ptr(call)?;
            // SAFETY: sp is a live Box::into_raw pointer; the cursor is
            // single-threaded per request so no aliasing &mut exists.
            let state = unsafe { &mut *sp };
            state.index = 0;
            cursor_advance_to_present(state);
            call.ret_null();
            Ok(())
        })
        // ── valid(): bool ──────────────────────────────────────────
        .method("valid")
        .returns(PhpType::Bool)
        .handler(|call| {
            let sp = cursor_state_ptr(call)?;
            // SAFETY: see rewind.
            let state = unsafe { &*sp };
            let valid = state.index < state.keys.len() && state.current.is_some();
            call.ret_bool(valid);
            Ok(())
        })
        // ── current(): mixed ───────────────────────────────────────
        .method("current")
        .returns(PhpType::Mixed)
        .handler(|call| {
            let sp = cursor_state_ptr(call)?;
            // SAFETY: see rewind. Clone the bytes so the borrow ends
            // before we touch `call` mutably.
            let bytes = unsafe { (*sp).current.clone() };
            match bytes {
                Some(b) if !b.is_empty() => {
                    let retval = call.retval_ptr();
                    let rc = unsafe {
                        bridge_ffi::oxphp_portable_deserialize(
                            b.as_ptr(),
                            b.len(),
                            1,
                            retval as *mut _,
                        )
                    };
                    if rc != 0 {
                        call.ret_null();
                    }
                }
                _ => call.ret_null(),
            }
            Ok(())
        })
        // ── key(): int|string ──────────────────────────────────────
        .method("key")
        .returns(PhpType::Union(vec![PhpType::Int, PhpType::String]))
        .handler(|call| {
            let sp = cursor_state_ptr(call)?;
            // SAFETY: see rewind. Read the key out before touching call.
            let key = unsafe {
                let state = &*sp;
                if state.index < state.keys.len() {
                    Some(state.keys[state.index].clone())
                } else {
                    None
                }
            };
            match key {
                Some(MapKey::Int(i)) => call.ret_long(i),
                // Binary-safe: keys are opaque bytes.
                Some(MapKey::Str(s)) => call.ret_bytes(&s),
                None => call.ret_null(),
            }
            Ok(())
        })
        // ── next(): void ───────────────────────────────────────────
        .method("next")
        .returns(PhpType::Void)
        .handler(|call| {
            let sp = cursor_state_ptr(call)?;
            // SAFETY: see rewind.
            let state = unsafe { &mut *sp };
            state.index += 1;
            cursor_advance_to_present(state);
            call.ret_null();
            Ok(())
        })
        .build()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ik(i: i64) -> MapKey {
        MapKey::Int(i)
    }
    fn sk(s: &str) -> MapKey {
        MapKey::from_str(s)
    }

    // ── Test helper: build an array SharedValue from str=>int pairs.
    impl SharedValue {
        fn from_pairs(pairs: &[(&str, i64)]) -> SharedValue {
            let mut arr = crate::plugins::ox_shared::value::SharedArray::default();
            for (k, v) in pairs {
                arr.str_keyed.push((Arc::from(*k), SharedValue::Long(*v)));
            }
            SharedValue::Array(Arc::new(arr))
        }
    }

    // ── basic CRUD ─────────────────────────────────────────────────

    #[test]
    fn new_unbounded() {
        let m = MapInner::new(None);
        assert_eq!(m.count(), 0);
        assert_eq!(m.max_entries(), None);
    }

    #[test]
    fn set_get_remove() {
        let m = MapInner::new(None);
        assert!(m.set(sk("a"), SharedValue::Long(1)).unwrap().is_none());
        assert!(matches!(m.get(&sk("a")), Some(SharedValue::Long(1))));
        let prev = m.set(sk("a"), SharedValue::Long(2)).unwrap();
        assert!(matches!(prev, Some(SharedValue::Long(1))));
        assert!(m.remove(&sk("a")));
        assert!(!m.remove(&sk("a")));
        assert!(m.get(&sk("a")).is_none());
    }

    #[test]
    fn set_rejects_null_value() {
        let m = MapInner::new(None);
        assert_eq!(
            m.set(sk("a"), SharedValue::Null).unwrap_err(),
            SharedError::Type
        );
        assert_eq!(m.count(), 0);
    }

    // ── MapKey distinctness (Task 1.1) ─────────────────────────────

    #[test]
    fn int_and_string_keys_are_distinct() {
        let m = MapInner::new(None);
        m.set(ik(123), SharedValue::String(Arc::from("int")))
            .unwrap();
        m.set(sk("123"), SharedValue::String(Arc::from("str")))
            .unwrap();
        assert_eq!(m.count(), 2);
        assert!(matches!(m.get(&ik(123)), Some(SharedValue::String(s)) if &*s == "int"));
        assert!(matches!(m.get(&sk("123")), Some(SharedValue::String(s)) if &*s == "str"));
    }

    #[test]
    fn binary_keys_round_trip() {
        let inner = MapInner::new(None);
        // Non-UTF-8 key bytes round-trip faithfully (no rejection, no lossy).
        inner
            .set(MapKey::from_bytes(b"\xff\xfe"), SharedValue::Long(1))
            .unwrap();
        assert!(matches!(
            inner.get(&MapKey::from_bytes(b"\xff\xfe")),
            Some(SharedValue::Long(1))
        ));
        // Distinct byte keys hash/compare distinctly, and an empty string
        // key is distinct from any non-empty byte key.
        assert_ne!(MapKey::from_bytes(b"\xff"), MapKey::from_bytes(b"\xfe"));
        assert_ne!(MapKey::from_bytes(b"\xff"), MapKey::from_str(""));
    }

    // ── striped counter (Task 1.2) ─────────────────────────────────

    #[test]
    fn striped_count_exact_when_quiescent() {
        let inner = MapInner::new(None);
        for i in 0..1000 {
            inner.set(MapKey::Int(i), SharedValue::Long(i)).unwrap();
        }
        assert_eq!(inner.count(), 1000);
        for i in 0..400 {
            inner.remove(&MapKey::Int(i));
        }
        assert_eq!(inner.count(), 600);
    }

    #[test]
    fn clear_resets_count() {
        let m = MapInner::new(None);
        for i in 0..50 {
            m.set(ik(i), SharedValue::Long(i)).unwrap();
        }
        assert_eq!(m.clear(), 50);
        assert_eq!(m.count(), 0);
    }

    // ── soft cap (Task 1.3) ────────────────────────────────────────

    #[test]
    fn soft_cap_rejects_new_key_allows_overwrite() {
        let inner = MapInner::new(Some(2));
        inner
            .set(MapKey::from_str("a"), SharedValue::Long(1))
            .unwrap();
        inner
            .set(MapKey::from_str("b"), SharedValue::Long(2))
            .unwrap();
        // overwrite existing — allowed at cap
        inner
            .set(MapKey::from_str("a"), SharedValue::Long(9))
            .unwrap();
        // new key at cap — rejected
        let e = inner
            .set(MapKey::from_str("c"), SharedValue::Long(3))
            .unwrap_err();
        assert_eq!(e, SharedError::CapacityExceeded);
    }

    #[test]
    fn cap_freed_by_remove() {
        let m = MapInner::new(Some(2));
        m.set(sk("a"), SharedValue::Long(1)).unwrap();
        m.set(sk("b"), SharedValue::Long(2)).unwrap();
        assert_eq!(
            m.set(sk("c"), SharedValue::Long(3)).unwrap_err(),
            SharedError::CapacityExceeded
        );
        m.remove(&sk("a"));
        m.set(sk("c"), SharedValue::Long(3)).unwrap();
        assert_eq!(m.count(), 2);
    }

    // ── compare_and_set (Task 1.4) ─────────────────────────────────

    #[test]
    fn cas_insert_replace_remove_via_null_sentinel() {
        let inner = MapInner::new(None);
        // insert iff absent
        assert!(inner
            .compare_and_set(MapKey::from_str("k"), None, Some(SharedValue::Long(1)))
            .unwrap());
        assert!(!inner
            .compare_and_set(MapKey::from_str("k"), None, Some(SharedValue::Long(9)))
            .unwrap());
        // replace iff == expected
        assert!(inner
            .compare_and_set(
                MapKey::from_str("k"),
                Some(SharedValue::Long(1)),
                Some(SharedValue::Long(2))
            )
            .unwrap());
        assert!(!inner
            .compare_and_set(
                MapKey::from_str("k"),
                Some(SharedValue::Long(1)),
                Some(SharedValue::Long(3))
            )
            .unwrap());
        // remove iff == expected
        assert!(inner
            .compare_and_set(MapKey::from_str("k"), Some(SharedValue::Long(2)), None)
            .unwrap());
        assert!(inner.get(&MapKey::from_str("k")).is_none());
    }

    #[test]
    fn cas_absent_to_absent_noop() {
        let m = MapInner::new(None);
        // absent → absent: true (no-op), and key stays absent
        assert!(m.compare_and_set(sk("x"), None, None).unwrap());
        assert!(m.get(&sk("x")).is_none());
        assert_eq!(m.count(), 0);
    }

    #[test]
    fn cas_array_equality_is_order_sensitive() {
        let inner = MapInner::new(None);
        let a = SharedValue::from_pairs(&[("x", 1), ("y", 2)]);
        let a_reordered = SharedValue::from_pairs(&[("y", 2), ("x", 1)]);
        inner.set(MapKey::from_str("k"), a.clone()).unwrap();
        assert!(!inner
            .compare_and_set(
                MapKey::from_str("k"),
                Some(a_reordered),
                Some(SharedValue::Long(0))
            )
            .unwrap());
        assert!(inner
            .compare_and_set(MapKey::from_str("k"), Some(a), Some(SharedValue::Long(0)))
            .unwrap());
    }

    #[test]
    fn cas_rejects_null_new_value() {
        let m = MapInner::new(None);
        // null `new` (non-absent sentinel) is unstorable.
        assert_eq!(
            m.compare_and_set(sk("k"), None, Some(SharedValue::Null))
                .unwrap_err(),
            SharedError::Type
        );
    }

    // ── swap / pop / set_if_absent (Task 1.5) ──────────────────────

    #[test]
    fn swap_and_pop_return_prev_or_none() {
        let inner = MapInner::new(None);
        assert!(inner
            .swap(MapKey::from_str("k"), SharedValue::Long(1))
            .unwrap()
            .is_none()); // was absent
        assert!(matches!(
            inner
                .swap(MapKey::from_str("k"), SharedValue::Long(2))
                .unwrap(),
            Some(SharedValue::Long(1))
        ));
        assert!(matches!(
            inner.pop(&MapKey::from_str("k")),
            Some(SharedValue::Long(2))
        ));
        assert!(inner.pop(&MapKey::from_str("k")).is_none());
    }

    #[test]
    fn set_if_absent_returns_prev() {
        let inner = MapInner::new(None);
        assert!(inner
            .set_if_absent(MapKey::from_str("k"), SharedValue::Long(1))
            .unwrap()
            .is_none()); // inserted
        assert!(matches!(
            inner
                .set_if_absent(MapKey::from_str("k"), SharedValue::Long(9))
                .unwrap(),
            Some(SharedValue::Long(1))
        ));
        // value not clobbered
        assert!(matches!(m_get(&inner, "k"), Some(SharedValue::Long(1))));
    }

    fn m_get(m: &MapInner, k: &str) -> Option<SharedValue> {
        m.get(&sk(k))
    }

    #[test]
    fn swap_rejects_null() {
        let m = MapInner::new(None);
        assert_eq!(
            m.swap(sk("k"), SharedValue::Null).unwrap_err(),
            SharedError::Type
        );
    }

    // ── key snapshot (forEach) ─────────────────────────────────────

    #[test]
    fn all_keys_covers_every_entry_once() {
        let inner = MapInner::new(None);
        for i in 0..200 {
            inner.set(MapKey::Int(i), SharedValue::Long(i)).unwrap();
        }
        let keys = inner.all_keys();
        assert_eq!(keys.len(), 200, "one entry per key, no duplicates");
        let seen: std::collections::HashSet<_> = keys.into_iter().collect();
        assert_eq!(seen.len(), 200);
        for i in 0..200 {
            assert!(seen.contains(&MapKey::Int(i)));
        }
    }

    // ── set_many_batch ─────────────────────────────────────────────

    #[test]
    fn set_many_batch_inserts_and_counts() {
        let m = MapInner::new(None);
        let batch = vec![
            (sk("a"), SharedValue::Long(1)),
            (ik(7), SharedValue::Long(2)),
            (sk("b"), SharedValue::Long(3)),
        ];
        assert_eq!(m.set_many_batch(batch).unwrap(), 3);
        assert_eq!(m.count(), 3);
        assert!(matches!(m.get(&ik(7)), Some(SharedValue::Long(2))));
    }

    #[test]
    fn set_many_batch_bails_on_cap() {
        let m = MapInner::new(Some(2));
        let batch = vec![
            (sk("a"), SharedValue::Long(1)),
            (sk("b"), SharedValue::Long(2)),
            (sk("c"), SharedValue::Long(3)),
        ];
        let (e, n) = m.set_many_batch(batch).unwrap_err();
        assert_eq!(e, SharedError::CapacityExceeded);
        assert_eq!(n, 2);
    }

    // ── SharedInner / downcast ─────────────────────────────────────

    #[test]
    fn shared_inner_type_and_snapshot() {
        let m = MapInner::new(Some(100));
        m.set(sk("a"), SharedValue::Long(1)).unwrap();
        m.set(sk("b"), SharedValue::Long(2)).unwrap();
        assert_eq!(m.type_tag(), SharedType::Map);
        match m.debug_snapshot() {
            SharedValue::Long(n) => assert_eq!(n, 2),
            other => panic!("expected Long, got {other:?}"),
        }
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
    fn mem_bytes_scales_with_entries() {
        let m = MapInner::new(None);
        let empty = m.mem_bytes();
        for i in 0..50 {
            m.set(ik(i), SharedValue::Long(i)).unwrap();
        }
        assert!(m.mem_bytes() > empty);
    }

    // ── content equality helper ────────────────────────────────────

    #[test]
    fn sv_content_eq_scalars_and_strings() {
        assert!(sv_content_eq(&SharedValue::Long(1), &SharedValue::Long(1)));
        assert!(!sv_content_eq(&SharedValue::Long(1), &SharedValue::Long(2)));
        assert!(sv_content_eq(
            &SharedValue::String(Arc::from("x")),
            &SharedValue::String(Arc::from("x"))
        ));
        assert!(!sv_content_eq(
            &SharedValue::String(Arc::from("x")),
            &SharedValue::String(Arc::from("y"))
        ));
        // cross-type scalar inequality (matches PHP ===)
        assert!(!sv_content_eq(
            &SharedValue::Long(1),
            &SharedValue::String(Arc::from("1"))
        ));
    }

    // ── registry-backed: retain/release + cycle detection ──────────

    use crate::plugins::ox_shared::config::{LockDiagnosticsLevel, SharedConfig};
    use crate::plugins::ox_shared::registry::{init_registry, registry, SharedRegistry};
    use crate::plugins::ox_shared::types::counter::CounterInner;
    use crate::plugins::ox_shared::value::SharedRefOwned;

    fn ensure_registry() -> &'static SharedRegistry {
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
            max_value_size: 1 << 20,
            poison_strict: false,
            lock_diagnostics: LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
            preview_string_limit: 256,
            preview_array_limit: 20,
        });
        registry()
    }

    fn make_mock_shared(reg: &'static SharedRegistry) -> (SharedValue, SharedId) {
        let arc = reg
            .insert(SharedType::Counter, Arc::new(CounterInner::new(0)))
            .expect("registry capacity should be sufficient for test");
        let id = arc.id;
        let sv = SharedValue::Shared(SharedRefOwned::from_arc(arc));
        (sv, id)
    }

    #[test]
    fn set_shared_retains_and_clear_releases() {
        let reg = ensure_registry();
        let (sv, id) = make_mock_shared(reg);
        assert!(reg.lookup(id).is_ok());
        let m = MapInner::new(None);
        m.set(sk("x"), sv).unwrap();
        assert!(reg.lookup(id).is_ok());
        m.clear();
        assert!(reg.lookup(id).is_err());
    }

    #[test]
    fn pop_hands_retain_to_caller() {
        let reg = ensure_registry();
        let (sv, id) = make_mock_shared(reg);
        let m = MapInner::new(None);
        m.set(sk("x"), sv).unwrap();
        let prev = m.pop(&sk("x")).expect("present");
        assert!(reg.lookup(id).is_ok(), "alive via prev's Arc");
        drop(prev);
        assert!(reg.lookup(id).is_err());
    }

    fn bootstrap_map(
        reg: &'static SharedRegistry,
        max: Option<usize>,
    ) -> (Arc<dyn SharedInner>, Arc<Entry>, SharedId) {
        let inner: Arc<dyn SharedInner> = Arc::new(MapInner::new(max));
        let entry = reg.insert(SharedType::Map, Arc::clone(&inner)).unwrap();
        let id = entry.id;
        let concrete = (*inner).as_any_map().unwrap();
        concrete.bind_id(id);
        (inner, entry, id)
    }

    fn shared_value_for(entry: &Arc<Entry>) -> SharedValue {
        SharedValue::Shared(SharedRefOwned::from_arc(Arc::clone(entry)))
    }

    #[test]
    fn direct_self_insert_is_rejected() {
        let reg = ensure_registry();
        let (map_arc, map_entry, _) = bootstrap_map(reg, None);
        let map = (*map_arc).as_any_map().unwrap();
        let self_ref = shared_value_for(&map_entry);
        assert!(matches!(
            map.set(sk("loop"), self_ref),
            Err(SharedError::Cycle)
        ));
        assert_eq!(map.count(), 0);
        drop(map_arc);
        drop(map_entry);
    }

    #[test]
    fn cas_propagates_cycle_rejection() {
        let reg = ensure_registry();
        let (a_arc, a_entry, _) = bootstrap_map(reg, None);
        let a = (*a_arc).as_any_map().unwrap();
        let rc = a.compare_and_set(sk("self"), None, Some(shared_value_for(&a_entry)));
        assert!(matches!(rc, Err(SharedError::Cycle)));
        assert_eq!(a.count(), 0);
        drop(a_arc);
        drop(a_entry);
    }

    #[test]
    fn two_map_cycle_via_shared_is_rejected() {
        let reg = ensure_registry();
        let (a_arc, a_entry, _) = bootstrap_map(reg, None);
        let (b_arc, b_entry, _) = bootstrap_map(reg, None);
        let a = (*a_arc).as_any_map().unwrap();
        let b = (*b_arc).as_any_map().unwrap();
        a.set(sk("b"), shared_value_for(&b_entry)).unwrap();
        assert!(matches!(
            b.set(sk("a"), shared_value_for(&a_entry)),
            Err(SharedError::Cycle)
        ));
        assert_eq!(b.count(), 0);
        assert_eq!(a.count(), 1);
        a.clear();
        drop(a_arc);
        drop(b_arc);
        drop(a_entry);
        drop(b_entry);
    }

    #[test]
    fn map_set_grows_registry_entry_bytes() {
        let reg = ensure_registry();
        let (map_arc, map_entry, _) = bootstrap_map(reg, None);
        let m = (*map_arc).as_any_map().unwrap();
        let baseline = map_entry.mem_bytes.load(Ordering::Relaxed);
        for i in 0..32i64 {
            m.set(ik(i), SharedValue::Long(i)).unwrap();
        }
        let grown = map_entry.mem_bytes.load(Ordering::Relaxed);
        assert!(grown > baseline);
        for i in 0..32i64 {
            m.remove(&ik(i));
        }
        assert_eq!(
            map_entry.mem_bytes.load(Ordering::Relaxed),
            baseline,
            "mem_bytes returns to baseline after symmetric removes"
        );
        m.clear();
        drop(map_arc);
        drop(map_entry);
    }

    // ── concurrency ────────────────────────────────────────────────

    #[test]
    fn concurrent_writers_count_exact() {
        use std::sync::Arc as StdArc;
        use std::thread;
        let m: StdArc<MapInner> = StdArc::new(MapInner::new(None));
        let mut handles = Vec::new();
        for t in 0..8 {
            let m = StdArc::clone(&m);
            handles.push(thread::spawn(move || {
                for i in 0..1000i64 {
                    m.set(sk(&format!("t{t}-k{i:04}")), SharedValue::Long(i))
                        .unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(m.count(), 8 * 1000);
    }

    #[test]
    fn clear_concurrent_with_insert_keeps_count_exact() {
        use std::sync::atomic::{AtomicBool, Ordering as O};
        use std::sync::Arc as StdArc;
        use std::thread;
        let m: StdArc<MapInner> = StdArc::new(MapInner::new(None));
        let stop = StdArc::new(AtomicBool::new(false));

        let mut handles = Vec::new();
        for t in 0..4 {
            let m = StdArc::clone(&m);
            let stop = StdArc::clone(&stop);
            handles.push(thread::spawn(move || {
                let mut i = 0i64;
                while !stop.load(O::Relaxed) {
                    m.set(sk(&format!("t{t}-k{i}")), SharedValue::Long(i))
                        .unwrap();
                    i += 1;
                }
            }));
        }

        // Hammer clear() concurrently with the writers. The old store(0)
        // reset desynced the striped counter against a racing insert.
        let mc = StdArc::clone(&m);
        for _ in 0..500 {
            mc.clear();
            std::thread::yield_now();
        }
        stop.store(true, O::Relaxed);
        for h in handles {
            h.join().unwrap();
        }

        // Quiescent: the striped count must equal the real entry count.
        let actual = m.entries.iter().count();
        assert_eq!(
            m.count(),
            actual,
            "striped count desynced from real entries after concurrent clear"
        );
    }

    // ── PHP class registration ─────────────────────────────────────

    #[test]
    fn register_class_emits_expected_surface() {
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

        let methods: std::collections::HashSet<&str> =
            map_class.methods.iter().map(|m| m.name.as_str()).collect();
        for expected in [
            "__construct",
            "get",
            "set",
            "setIfAbsent",
            "setMany",
            "getMany",
            "remove",
            "removeMany",
            "clear",
            "swap",
            "pop",
            "compareAndSet",
            "count",
            "maxEntries",
            "forEach",
            "id",
        ] {
            assert!(
                methods.contains(expected),
                "missing method `{expected}` in Map registration"
            );
        }
        // Removed surface must be gone.
        for removed in ["has", "update", "getOrSet", "updateMany", "keys", "trySet"] {
            assert!(
                !methods.contains(removed),
                "removed method `{removed}` is still registered"
            );
        }
        // No longer Countable.
        assert!(!map_class.interfaces.iter().any(|i| i == "Countable"));
        assert!(map_class
            .interfaces
            .iter()
            .any(|i| i == "OxPHP\\Shared\\Shareable"));

        // KeyCursor registered as an Iterator.
        let cursor = php_classes
            .iter()
            .find(|c| c.fqn == KEY_CURSOR_FQN)
            .expect("Map\\KeyCursor must be registered");
        assert!(cursor.interfaces.iter().any(|i| i == "Iterator"));
        let cursor_methods: std::collections::HashSet<&str> =
            cursor.methods.iter().map(|m| m.name.as_str()).collect();
        for expected in ["rewind", "valid", "current", "key", "next"] {
            assert!(
                cursor_methods.contains(expected),
                "KeyCursor missing `{expected}`"
            );
        }
    }

    // ── FFI round-trip (new tagged-key signatures) ─────────────────

    struct TestMap(*const Entry);
    impl TestMap {
        fn new(max_entries: i64) -> Self {
            ensure_registry();
            let mut ptr: *const Entry = std::ptr::null();
            let rc = unsafe { oxphp_shared_map_create(max_entries, &mut ptr) };
            assert_eq!(rc, 0);
            assert!(!ptr.is_null());
            Self(ptr)
        }
        fn entry(&self) -> *const Entry {
            self.0
        }
    }
    impl Drop for TestMap {
        fn drop(&mut self) {
            unsafe { crate::plugins::ox_shared::registry::oxphp_shared_handle_drop(self.0) };
        }
    }

    fn ffi_set_int_key(id: *const Entry, key: i64, v: &SharedValue) -> c_int {
        let buf = sv_to_portbuf(v);
        unsafe {
            oxphp_shared_map_set(
                id,
                KEY_KIND_INT,
                key,
                std::ptr::null(),
                0,
                buf.as_ptr(),
                buf.len(),
            )
        }
    }

    fn ffi_set_str_key(id: *const Entry, key: &str, v: &SharedValue) -> c_int {
        let buf = sv_to_portbuf(v);
        let kb = key.as_bytes();
        unsafe {
            oxphp_shared_map_set(
                id,
                KEY_KIND_STR,
                0,
                kb.as_ptr(),
                kb.len(),
                buf.as_ptr(),
                buf.len(),
            )
        }
    }

    #[test]
    fn ffi_set_get_int_and_str_keys() {
        let m = TestMap::new(0);
        let id = m.entry();
        assert_eq!(ffi_set_int_key(id, 123, &SharedValue::Long(7)), 0);
        assert_eq!(
            ffi_set_str_key(id, "123", &SharedValue::String(Arc::from("s"))),
            0
        );

        // count == 2 (distinct keys)
        let mut c: u64 = 0;
        unsafe { oxphp_shared_map_count(id, &mut c) };
        assert_eq!(c, 2);

        // get int key
        let mut buf: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        let mut absent: c_int = 0;
        let rc = unsafe {
            oxphp_shared_map_get(
                id,
                KEY_KIND_INT,
                123,
                std::ptr::null(),
                0,
                &mut buf,
                &mut len,
                &mut absent,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(absent, 0);
        let decoded = portbuf_to_sv(unsafe { std::slice::from_raw_parts(buf, len) }).unwrap();
        assert!(matches!(
            decoded,
            crate::plugins::ox_shared::value::SharedValueRaw::Long(7)
        ));
        unsafe { libc::free(buf as *mut libc::c_void) };
    }

    #[test]
    fn ffi_remove_out_bool() {
        let m = TestMap::new(0);
        let id = m.entry();
        ffi_set_str_key(id, "gone", &SharedValue::Long(1));
        let mut existed: c_int = 0;
        let kb = b"gone";
        let rc = unsafe {
            oxphp_shared_map_remove(id, KEY_KIND_STR, 0, kb.as_ptr(), kb.len(), &mut existed)
        };
        assert_eq!(rc, 0);
        assert_eq!(existed, 1);
        let rc = unsafe {
            oxphp_shared_map_remove(id, KEY_KIND_STR, 0, kb.as_ptr(), kb.len(), &mut existed)
        };
        assert_eq!(rc, 0);
        assert_eq!(existed, 0);
    }

    #[test]
    fn ffi_swap_pop_return_prev() {
        let m = TestMap::new(0);
        let id = m.entry();
        let v1 = sv_to_portbuf(&SharedValue::Long(1));
        let v2 = sv_to_portbuf(&SharedValue::Long(2));
        let kb = b"k";

        // swap on absent → out_absent=1
        let (mut pbuf, mut plen, mut absent) = (std::ptr::null_mut(), 0usize, 0 as c_int);
        let rc = unsafe {
            oxphp_shared_map_swap(
                id,
                KEY_KIND_STR,
                0,
                kb.as_ptr(),
                kb.len(),
                v1.as_ptr(),
                v1.len(),
                &mut pbuf,
                &mut plen,
                &mut absent,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(absent, 1);

        // swap again → returns prev = 1
        let rc = unsafe {
            oxphp_shared_map_swap(
                id,
                KEY_KIND_STR,
                0,
                kb.as_ptr(),
                kb.len(),
                v2.as_ptr(),
                v2.len(),
                &mut pbuf,
                &mut plen,
                &mut absent,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(absent, 0);
        let prev = portbuf_to_sv(unsafe { std::slice::from_raw_parts(pbuf, plen) }).unwrap();
        assert!(matches!(
            prev,
            crate::plugins::ox_shared::value::SharedValueRaw::Long(1)
        ));
        unsafe { libc::free(pbuf as *mut libc::c_void) };

        // pop → returns prev = 2
        let rc = unsafe {
            oxphp_shared_map_pop(
                id,
                KEY_KIND_STR,
                0,
                kb.as_ptr(),
                kb.len(),
                &mut pbuf,
                &mut plen,
                &mut absent,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(absent, 0);
        let prev = portbuf_to_sv(unsafe { std::slice::from_raw_parts(pbuf, plen) }).unwrap();
        assert!(matches!(
            prev,
            crate::plugins::ox_shared::value::SharedValueRaw::Long(2)
        ));
        unsafe { libc::free(pbuf as *mut libc::c_void) };
    }

    #[test]
    fn ffi_set_if_absent_out_params() {
        let m = TestMap::new(0);
        let id = m.entry();
        let v = sv_to_portbuf(&SharedValue::Long(1));
        let kb = b"once";
        let (mut pbuf, mut plen, mut absent) = (std::ptr::null_mut(), 0usize, 0 as c_int);

        let rc = unsafe {
            oxphp_shared_map_set_if_absent(
                id,
                KEY_KIND_STR,
                0,
                kb.as_ptr(),
                kb.len(),
                v.as_ptr(),
                v.len(),
                &mut pbuf,
                &mut plen,
                &mut absent,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(absent, 1, "first insert → absent flag set, no prev");

        let rc = unsafe {
            oxphp_shared_map_set_if_absent(
                id,
                KEY_KIND_STR,
                0,
                kb.as_ptr(),
                kb.len(),
                v.as_ptr(),
                v.len(),
                &mut pbuf,
                &mut plen,
                &mut absent,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(absent, 0, "second call returns prev");
        let prev = portbuf_to_sv(unsafe { std::slice::from_raw_parts(pbuf, plen) }).unwrap();
        assert!(matches!(
            prev,
            crate::plugins::ox_shared::value::SharedValueRaw::Long(1)
        ));
        unsafe { libc::free(pbuf as *mut libc::c_void) };
    }

    #[test]
    fn ffi_compare_and_set_sentinels() {
        let m = TestMap::new(0);
        let id = m.entry();
        let kb = b"k";
        let one = sv_to_portbuf(&SharedValue::Long(1));
        let two = sv_to_portbuf(&SharedValue::Long(2));
        let mut swapped: c_int = 0;

        // insert iff absent: expected absent, new=1
        let rc = unsafe {
            oxphp_shared_map_compare_and_set(
                id,
                KEY_KIND_STR,
                0,
                kb.as_ptr(),
                kb.len(),
                std::ptr::null(),
                0,
                1, // expected_is_absent
                one.as_ptr(),
                one.len(),
                0, // new present
                &mut swapped,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(swapped, 1);

        // replace iff == 1: expected=1, new=2
        let rc = unsafe {
            oxphp_shared_map_compare_and_set(
                id,
                KEY_KIND_STR,
                0,
                kb.as_ptr(),
                kb.len(),
                one.as_ptr(),
                one.len(),
                0,
                two.as_ptr(),
                two.len(),
                0,
                &mut swapped,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(swapped, 1);

        // conditional remove: expected=2, new absent
        let rc = unsafe {
            oxphp_shared_map_compare_and_set(
                id,
                KEY_KIND_STR,
                0,
                kb.as_ptr(),
                kb.len(),
                two.as_ptr(),
                two.len(),
                0,
                std::ptr::null(),
                0,
                1, // new_is_absent
                &mut swapped,
            )
        };
        assert_eq!(rc, 0);
        assert_eq!(swapped, 1);

        let mut c: u64 = 0;
        unsafe { oxphp_shared_map_count(id, &mut c) };
        assert_eq!(c, 0);
    }

    #[test]
    fn ffi_set_many_nested_pairs() {
        let m = TestMap::new(0);
        let id = m.entry();
        let pairs = vec![
            IterPair {
                key: sk("a"),
                value: sv_to_portbuf(&SharedValue::Long(1)),
            },
            IterPair {
                key: ik(7),
                value: sv_to_portbuf(&SharedValue::Long(2)),
            },
        ];
        let buf = encode_pairs_portbuf(&pairs);
        let mut inserted: u64 = 0;
        let rc = unsafe { oxphp_shared_map_set_many(id, buf.as_ptr(), buf.len(), &mut inserted) };
        assert_eq!(rc, 0);
        assert_eq!(inserted, 2);

        let mut c: u64 = 0;
        unsafe { oxphp_shared_map_count(id, &mut c) };
        assert_eq!(c, 2);
    }

    #[test]
    fn ffi_remove_many_counts_hits() {
        let m = TestMap::new(0);
        let id = m.entry();
        ffi_set_str_key(id, "a", &SharedValue::Long(1));
        ffi_set_str_key(id, "b", &SharedValue::Long(2));
        let keys = vec![sk("a"), sk("missing")];
        let buf = encode_keys_portbuf(&keys);
        let mut removed: u64 = 0;
        let rc = unsafe { oxphp_shared_map_remove_many(id, buf.as_ptr(), buf.len(), &mut removed) };
        assert_eq!(rc, 0);
        assert_eq!(removed, 1);
    }

    #[test]
    fn ffi_all_keys_round_trip() {
        let m = TestMap::new(0);
        let id = m.entry();
        for i in 0..40i64 {
            ffi_set_int_key(id, i, &SharedValue::Long(i));
        }
        let mut buf: *mut u8 = std::ptr::null_mut();
        let mut len = 0usize;
        let rc = unsafe { oxphp_shared_map_all_keys(id, &mut buf, &mut len) };
        assert_eq!(rc, 0);
        let mut seen = std::collections::HashSet::new();
        for k in decode_key_array(buf, len) {
            if let MapKey::Int(i) = k {
                seen.insert(i);
            }
        }
        if !buf.is_null() {
            unsafe { libc::free(buf as *mut libc::c_void) };
        }
        assert_eq!(seen.len(), 40);
    }

    #[test]
    fn ffi_clear_returns_count() {
        let m = TestMap::new(0);
        let id = m.entry();
        for i in 0..5i64 {
            ffi_set_int_key(id, i, &SharedValue::Long(i));
        }
        let mut removed: u64 = 0;
        let rc = unsafe { oxphp_shared_map_clear(id, &mut removed) };
        assert_eq!(rc, 0);
        assert_eq!(removed, 5);
        let mut c: u64 = 0;
        unsafe { oxphp_shared_map_count(id, &mut c) };
        assert_eq!(c, 0);
    }

    #[test]
    fn ffi_set_rejects_null_value() {
        let m = TestMap::new(0);
        let id = m.entry();
        let rc = ffi_set_str_key(id, "k", &SharedValue::Null);
        assert_eq!(rc, SharedError::Type.code());
    }

    #[test]
    fn ffi_set_cap_rejects() {
        let m = TestMap::new(1);
        let id = m.entry();
        assert_eq!(ffi_set_str_key(id, "a", &SharedValue::Long(1)), 0);
        assert_eq!(
            ffi_set_str_key(id, "b", &SharedValue::Long(2)),
            SharedError::CapacityExceeded.code()
        );
    }
}
