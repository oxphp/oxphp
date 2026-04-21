//! Profiler filter resolution + registry.
//!
//! At observer init time the C bridge asks Rust to resolve the
//! four filter attributes (`#[OxPHP\Profile\Profile]`,
//! `#[OxPHP\Profile\Exclude]`, `#[OxPHP\Profile\Sample(rate)]`,
//! `#[OxPHP\Profile\Tag(key, value)]`) for a given `zend_function`.
//! Rust returns a stable `spec_id` (32-bit handle, 0 = no filter)
//! plus a "decision quad" (excluded / force_profile / has_sample /
//! sample_rate) that the C side caches per `(fn, thread)` for
//! hot-path consultation in begin/end.
//!
//! `apply_events` (in `mod.rs`) reads the `spec_id` from each BEGIN
//! event's `reserved2` slot and, for non-zero IDs, looks up the
//! `FilterSpec` here to attach static tags to the freshly-pushed
//! span.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::CStr;
#[cfg(feature = "php")]
use std::ffi::CString;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use arc_swap::ArcSwap;
use parking_lot::Mutex;

const ATTR_PROFILE: &str = "OxPHP\\Profile\\Profile";
const ATTR_EXCLUDE: &str = "OxPHP\\Profile\\Exclude";
const ATTR_SAMPLE: &str = "OxPHP\\Profile\\Sample";
const ATTR_TAG: &str = "OxPHP\\Profile\\Tag";

/// Per-spec cap on accumulated tags. Class + method tags merge into
/// one list. 32 leaves room for layered framework tagging while
/// bounding worst-case memory at ~32 × 64 B ≈ 2 KiB / spec.
const MAX_TAGS_PER_SPEC: usize = 32;

/// Resolved filter spec for one `zend_function`. Owned by the
/// global registry as `Arc<FilterSpec>`; readers on the hot path
/// clone the `Arc` (one atomic bump) instead of deep-copying the
/// struct.
///
/// `tags` is an `Arc<[...]>` slice of `(Arc<str>, Arc<str>)` pairs
/// so that attaching the spec's static tags to a span is a single
/// `Arc::clone` of the slice handle (plus per-pair Arc bumps when
/// the span appends them to its own attribute list).
#[derive(Debug, Clone, Default)]
pub struct FilterSpec {
    pub force_profile: bool,
    pub excluded: bool,
    /// `None` = no `#[Sample]`; `Some(r)` with `r ∈ [0, 1]`.
    pub sample_rate: Option<f32>,
    /// Class tags first, then function tags. Capped at
    /// `MAX_TAGS_PER_SPEC`.
    pub tags: Arc<[(Arc<str>, Arc<str>)]>,
}

impl FilterSpec {
    /// Compose class-level and method-level specs per spec §6:
    /// - `Exclude` wins (OR-merge).
    /// - `force_profile` OR-merges (either side opts in).
    /// - `sample_rate`: method takes precedence; `None` falls
    ///   through to the class value.
    /// - Tags accumulate, class first then method, capped.
    pub fn merge_with_class(class_spec: FilterSpec, method_spec: FilterSpec) -> FilterSpec {
        let class_len = class_spec.tags.len();
        let method_len = method_spec.tags.len();
        let total = (class_len + method_len).min(MAX_TAGS_PER_SPEC);
        let mut merged: Vec<(Arc<str>, Arc<str>)> = Vec::with_capacity(total);
        for pair in class_spec.tags.iter() {
            if merged.len() >= MAX_TAGS_PER_SPEC {
                break;
            }
            merged.push(pair.clone());
        }
        for pair in method_spec.tags.iter() {
            if merged.len() >= MAX_TAGS_PER_SPEC {
                break;
            }
            merged.push(pair.clone());
        }
        FilterSpec {
            force_profile: method_spec.force_profile || class_spec.force_profile,
            excluded: method_spec.excluded || class_spec.excluded,
            sample_rate: method_spec.sample_rate.or(class_spec.sample_rate),
            tags: Arc::from(merged),
        }
    }

    /// True when the spec carries no information — caller can skip
    /// interning entirely (returns `spec_id = 0`).
    fn is_empty(&self) -> bool {
        !self.force_profile && !self.excluded && self.sample_rate.is_none() && self.tags.is_empty()
    }
}

type FilterMap = HashMap<u32, Arc<FilterSpec>>;

/// Copy-on-write registry of interned `FilterSpec`s.
///
/// Readers hit `load()` — one relaxed atomic — and get an
/// `Arc<HashMap<...>>` snapshot that outlives any concurrent
/// writer. Writers clone the current map, mutate the copy, and
/// `store` a new `Arc`. Registration happens at observer init
/// (once per function, once per worker thread in the common case),
/// so the O(N) copy-on-write cost stays amortised even under
/// heavy observation.
fn registry() -> &'static ArcSwap<FilterMap> {
    static REG: OnceLock<ArcSwap<FilterMap>> = OnceLock::new();
    REG.get_or_init(|| ArcSwap::from_pointee(FilterMap::default()))
}

/// Spec_id allocator. Starts at 1; 0 reserved for "no filter".
static SPEC_ID_NEXT: AtomicU32 = AtomicU32::new(1);

/// Serialises `intern` writers so concurrent copy-on-write updates
/// of the `ArcSwap` registry don't drop entries (last-writer-wins
/// on unsynchronised `load → clone → store`). Readers stay lock-free.
static FILTER_WRITE_MUTEX: Mutex<()> = Mutex::new(());

/// Look up a `FilterSpec` by handle. Returns `None` for spec_id 0
/// (no filter) or unknown IDs. The returned `Arc` shares the
/// registry entry — readers do not clone the spec contents.
pub fn get_spec(spec_id: u32) -> Option<Arc<FilterSpec>> {
    if spec_id == 0 {
        return None;
    }
    registry().load().get(&spec_id).cloned()
}

/// Insert a spec into the global registry, returning its handle.
/// Copy-on-write: snapshots the current map, inserts, and
/// atomically swaps. No de-duplication — two functions with
/// identical specs get distinct IDs. De-dup is a follow-up if
/// memory becomes a concern.
fn intern(spec: FilterSpec) -> u32 {
    let _write_guard = FILTER_WRITE_MUTEX.lock();
    let id = SPEC_ID_NEXT.fetch_add(1, Ordering::Relaxed);
    let arc_spec = Arc::new(spec);
    let reg = registry();
    let current = reg.load();
    let mut next = (**current).clone();
    next.insert(id, arc_spec);
    reg.store(Arc::new(next));
    id
}

/// Resolve filter attributes for one scope (class or function)
/// given the C-side context. Walks the attribute name list, calling
/// the bridge's arg-reader helpers as needed.
fn resolve_one_scope(
    attr_names: &[*const std::os::raw::c_char],
    is_class_scope: i32,
    ctx: *mut std::os::raw::c_void,
) -> FilterSpec {
    // Without `feature = "php"` the bridge arg-readers are absent
    // and the resolver only sets the boolean opt-in flags from the
    // attribute-name list. The tag/sample occurrence counters are
    // therefore php-gated (never read in the host build).
    #[cfg(not(feature = "php"))]
    let _ = (is_class_scope, ctx);

    let mut spec = FilterSpec::default();
    // Build tags into a Vec then freeze to `Arc<[...]>` once at the
    // end — avoids repeatedly re-allocating an Arc<[..]> per push.
    let mut tags: Vec<(Arc<str>, Arc<str>)> = Vec::new();
    #[cfg(feature = "php")]
    let mut tag_idx: u32 = 0;
    #[cfg(feature = "php")]
    let mut sample_idx: u32 = 0;

    for &name_ptr in attr_names {
        if name_ptr.is_null() {
            continue;
        }
        // SAFETY: name_ptr comes from the C side's attr->name (zend_string),
        // valid for the duration of the resolve call.
        let name = unsafe { CStr::from_ptr(name_ptr) }.to_string_lossy();
        match name.as_ref() {
            ATTR_PROFILE => {
                spec.force_profile = true;
            }
            ATTR_EXCLUDE => {
                spec.excluded = true;
            }
            ATTR_SAMPLE => {
                #[cfg(feature = "php")]
                {
                    let cname = CString::new(ATTR_SAMPLE).expect("ATTR_SAMPLE has no NULs");
                    let mut rate = 1.0_f64;
                    let ok = unsafe {
                        crate::php::bindings::oxphp_bridge_read_attr_arg_double(
                            ctx,
                            is_class_scope,
                            cname.as_ptr(),
                            sample_idx,
                            0,
                            &mut rate as *mut f64,
                        )
                    };
                    if ok != 0 {
                        spec.sample_rate = Some(rate.clamp(0.0, 1.0) as f32);
                    }
                    sample_idx += 1;
                }
                // In host build, presence of #[Sample] alone (without
                // arg readability) makes the spec non-empty — record
                // a sentinel rate of 1.0 so the spec actually interns.
                #[cfg(not(feature = "php"))]
                {
                    spec.sample_rate = Some(1.0);
                }
            }
            ATTR_TAG => {
                #[cfg(feature = "php")]
                {
                    let cname = CString::new(ATTR_TAG).expect("ATTR_TAG has no NULs");
                    let mut key_buf = [0 as std::os::raw::c_char; 128];
                    let mut val_buf = [0 as std::os::raw::c_char; 256];
                    let (key_len, val_len) = unsafe {
                        let kl = crate::php::bindings::oxphp_bridge_read_attr_arg_str(
                            ctx,
                            is_class_scope,
                            cname.as_ptr(),
                            tag_idx,
                            0,
                            key_buf.as_mut_ptr(),
                            key_buf.len(),
                        );
                        let vl = crate::php::bindings::oxphp_bridge_read_attr_arg_str(
                            ctx,
                            is_class_scope,
                            cname.as_ptr(),
                            tag_idx,
                            1,
                            val_buf.as_mut_ptr(),
                            val_buf.len(),
                        );
                        (kl, vl)
                    };
                    if key_len > 0 {
                        let key: Arc<str> = Arc::from(
                            unsafe { CStr::from_ptr(key_buf.as_ptr()) }
                                .to_string_lossy()
                                .as_ref(),
                        );
                        let val: Arc<str> = if val_len > 0 {
                            Arc::from(
                                unsafe { CStr::from_ptr(val_buf.as_ptr()) }
                                    .to_string_lossy()
                                    .as_ref(),
                            )
                        } else {
                            Arc::from("")
                        };
                        if tags.len() < MAX_TAGS_PER_SPEC {
                            tags.push((key, val));
                        }
                    }
                    tag_idx += 1;
                }
                // Host build: presence of #[Tag] alone (without arg
                // readability) records a placeholder so the spec is
                // non-empty and tests can verify the resolver wiring.
                #[cfg(not(feature = "php"))]
                {
                    if tags.len() < MAX_TAGS_PER_SPEC {
                        tags.push((Arc::from(""), Arc::from("")));
                    }
                }
            }
            _ => {} // not a profiler filter attr — ignore
        }
    }
    if !tags.is_empty() {
        spec.tags = Arc::from(tags);
    }
    spec
}

/// FFI entry registered with the C bridge at plugin init. The
/// observer init callback invokes this on first observation of any
/// function whose attributes include at least one `OxPHP\Profile\*`
/// name. Returns spec_id (0 if the resolved spec carries no filter
/// information after composition).
///
/// Output params receive the four hot-path decision values so the
/// C cache entry mirrors them and begin/end can decide without
/// re-entering Rust.
///
/// # Safety
/// All input pointers come from C-side TLS / stack and are valid
/// for the duration of the call. Output pointers must be writable.
#[no_mangle]
pub unsafe extern "C" fn oxphp_profiler_resolve_filter(
    _fn_id: usize,
    class_attr_names: *const *const std::os::raw::c_char,
    class_attr_count: u32,
    fn_attr_names: *const *const std::os::raw::c_char,
    fn_attr_count: u32,
    ctx: *mut std::os::raw::c_void,
    out_excluded: *mut u8,
    out_force_profile: *mut u8,
    out_has_sample: *mut u8,
    out_sample_rate: *mut f32,
) -> u32 {
    let class_slice: &[*const std::os::raw::c_char] = if class_attr_count == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(class_attr_names, class_attr_count as usize)
    };
    let fn_slice: &[*const std::os::raw::c_char] = if fn_attr_count == 0 {
        &[]
    } else {
        std::slice::from_raw_parts(fn_attr_names, fn_attr_count as usize)
    };

    let class_spec = resolve_one_scope(class_slice, 1, ctx);
    let fn_spec = resolve_one_scope(fn_slice, 0, ctx);
    let merged = FilterSpec::merge_with_class(class_spec, fn_spec);

    // Mirror the decision quad to the C cache entry.
    *out_excluded = u8::from(merged.excluded);
    *out_force_profile = u8::from(merged.force_profile);
    *out_has_sample = u8::from(merged.sample_rate.is_some());
    *out_sample_rate = merged.sample_rate.unwrap_or(0.0);

    if merged.is_empty() {
        0
    } else {
        intern(merged)
    }
}

// ─── PRNG for #[Sample] ────────────────────────────────────────

/// `xoshiro256**` PRNG state. Per-thread, lazily seeded from
/// `getrandom` on first use.
struct Xoshiro256 {
    state: [u64; 4],
}

impl Xoshiro256 {
    fn new() -> Self {
        let mut seed = [0u8; 32];
        let _ = getrandom::getrandom(&mut seed);
        let mut s = [0u64; 4];
        for (i, chunk) in seed.chunks_exact(8).enumerate() {
            s[i] = u64::from_le_bytes(chunk.try_into().unwrap());
        }
        // xoshiro requires non-zero state.
        if s == [0, 0, 0, 0] {
            s = [1, 2, 3, 4];
        }
        Self { state: s }
    }

    fn next_u64(&mut self) -> u64 {
        let result = self.state[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = self.state[1] << 17;
        self.state[2] ^= self.state[0];
        self.state[3] ^= self.state[1];
        self.state[1] ^= self.state[2];
        self.state[0] ^= self.state[3];
        self.state[2] ^= t;
        self.state[3] = self.state[3].rotate_left(45);
        result
    }

    /// Uniform `[0, 1)` with 24-bit precision (enough for sample-rate
    /// decisions; floor of 1/2^24 ≈ 6e-8 granularity).
    fn next_uniform_f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) * (1.0_f32 / (1_u32 << 24) as f32)
    }
}

thread_local! {
    static PRNG: RefCell<Xoshiro256> = RefCell::new(Xoshiro256::new());
}

/// Roll the per-thread PRNG. Returns `true` if `next_uniform_f32() <
/// rate`. Hot-path optimisations: `rate <= 0.0` always false (no
/// roll); `rate >= 1.0` always true (no roll).
pub fn sample_hit(rate: f32) -> bool {
    if rate <= 0.0 {
        return false;
    }
    if rate >= 1.0 {
        return true;
    }
    PRNG.with(|cell| cell.borrow_mut().next_uniform_f32() < rate)
}

/// FFI entry — called by the C-side `oxphp_profiler_begin` for
/// `#[Sample(rate)]` decisions. Returns 1 if the call should be
/// captured, 0 if it should be skipped.
#[cfg(feature = "php")]
#[no_mangle]
pub extern "C" fn oxphp_profiler_sample_hit(rate: f32) -> u8 {
    u8::from(sample_hit(rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_spec_detected() {
        assert!(FilterSpec::default().is_empty());
        assert!(!FilterSpec {
            force_profile: true,
            ..Default::default()
        }
        .is_empty());
    }

    #[test]
    fn merge_method_excluded_overrides_class_force_profile() {
        let class = FilterSpec {
            force_profile: true,
            ..Default::default()
        };
        let method = FilterSpec {
            excluded: true,
            ..Default::default()
        };
        let merged = FilterSpec::merge_with_class(class, method);
        assert!(merged.excluded);
        // force_profile OR-merges, so it stays true even though
        // method didn't set it. Begin/end short-circuits on Exclude
        // before consulting force_profile, so this is correct.
        assert!(merged.force_profile);
    }

    #[test]
    fn merge_sample_rate_method_wins_when_set() {
        let class = FilterSpec {
            sample_rate: Some(0.5),
            ..Default::default()
        };
        let method = FilterSpec {
            sample_rate: Some(0.1),
            ..Default::default()
        };
        let merged = FilterSpec::merge_with_class(class, method);
        assert_eq!(merged.sample_rate, Some(0.1));
    }

    #[test]
    fn merge_sample_rate_falls_through_to_class_when_method_unset() {
        let class = FilterSpec {
            sample_rate: Some(0.5),
            ..Default::default()
        };
        let method = FilterSpec::default();
        let merged = FilterSpec::merge_with_class(class, method);
        assert_eq!(merged.sample_rate, Some(0.5));
    }

    #[test]
    fn merge_tags_preserve_order_class_first() {
        let class = FilterSpec {
            tags: Arc::from(vec![(Arc::<str>::from("env"), Arc::<str>::from("prod"))]),
            ..Default::default()
        };
        let method = FilterSpec {
            tags: Arc::from(vec![(Arc::<str>::from("op"), Arc::<str>::from("select"))]),
            ..Default::default()
        };
        let merged = FilterSpec::merge_with_class(class, method);
        assert_eq!(merged.tags.len(), 2);
        assert_eq!(merged.tags[0].0.as_ref(), "env");
        assert_eq!(merged.tags[0].1.as_ref(), "prod");
        assert_eq!(merged.tags[1].0.as_ref(), "op");
        assert_eq!(merged.tags[1].1.as_ref(), "select");
    }

    #[test]
    fn merge_tags_capped_at_max() {
        let class_tags: Vec<(Arc<str>, Arc<str>)> = (0..40)
            .map(|i| {
                (
                    Arc::<str>::from(format!("k{i}").as_str()),
                    Arc::<str>::from(format!("v{i}").as_str()),
                )
            })
            .collect();
        let class = FilterSpec {
            tags: Arc::from(class_tags),
            ..Default::default()
        };
        let method = FilterSpec::default();
        let merged = FilterSpec::merge_with_class(class, method);
        assert_eq!(merged.tags.len(), MAX_TAGS_PER_SPEC);
    }

    #[test]
    fn intern_assigns_unique_ids_starting_at_1() {
        let id_a = intern(FilterSpec {
            force_profile: true,
            ..Default::default()
        });
        let id_b = intern(FilterSpec {
            excluded: true,
            ..Default::default()
        });
        assert!(id_a >= 1);
        assert_ne!(id_a, id_b);
        assert!(get_spec(id_a).unwrap().force_profile);
        assert!(get_spec(id_b).unwrap().excluded);
        assert!(get_spec(0).is_none(), "spec_id 0 reserved for no-filter");
    }

    #[test]
    fn resolver_returns_zero_for_empty_attribute_lists() {
        let mut excluded = 0u8;
        let mut force = 0u8;
        let mut has_sample = 0u8;
        let mut rate = 0.0_f32;
        let result = unsafe {
            oxphp_profiler_resolve_filter(
                0xdead_beef,
                std::ptr::null(),
                0,
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
                &mut excluded,
                &mut force,
                &mut has_sample,
                &mut rate,
            )
        };
        assert_eq!(result, 0);
        assert_eq!(excluded, 0);
        assert_eq!(force, 0);
        assert_eq!(has_sample, 0);
    }

    #[test]
    fn sample_hit_zero_always_false() {
        for _ in 0..50 {
            assert!(!sample_hit(0.0));
        }
    }

    #[test]
    fn sample_hit_one_always_true() {
        for _ in 0..50 {
            assert!(sample_hit(1.0));
        }
    }

    #[test]
    fn sample_hit_half_in_expected_range() {
        let mut hits = 0;
        for _ in 0..1000 {
            if sample_hit(0.5) {
                hits += 1;
            }
        }
        assert!(
            hits > 350 && hits < 650,
            "expected ~500 ± 150 hits, got {hits}"
        );
    }
}
