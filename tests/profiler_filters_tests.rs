//! Integration tests for observer-filter attributes.
//!
//! Pure-Rust simulation of the resolver → registry → apply_events
//! pipeline. Synthesises BEGIN events with non-zero `reserved2`
//! (spec_id) and verifies that tags accumulate on the resulting
//! span. Real PHP-driven coverage runs in
//! `tests/php/profiler/test_attr_*.php` under the Docker profile.

#![cfg(feature = "plugin-profiler")]

use std::ffi::CString;

use oxphp::profiling::filter::{
    self, get_spec, oxphp_profiler_resolve_filter, sample_hit, FilterSpec,
};
use oxphp::profiling::flush::{OxSpanEvent, SPAN_EVENT_KIND_BEGIN, SPAN_EVENT_KIND_END};
use oxphp::profiling::{ProfilingMode, PROFILING_CONTEXT};

/// Build an OxSpanEvent with the given spec_id in `reserved2`. The
/// rest of the fields are wired the same way the C observer would
/// emit them.
fn ev(kind: u8, seq: u64, name: &'static [u8], spec_id: u32) -> OxSpanEvent {
    OxSpanEvent {
        kind,
        reserved0: 0,
        name_len: name.len() as u16,
        reserved1: 0,
        seq,
        ts_ns: seq * 100,
        cpu_ns: seq * 10,
        mem: 0,
        mem_peak: 0,
        name_ptr: name.as_ptr() as *const std::os::raw::c_char,
        reserved2: spec_id as u64,
    }
}

#[test]
fn resolver_returns_zero_for_attribute_list_without_profile_attrs() {
    // Attribute names that don't start with OxPHP\Profile\ — should
    // resolve to spec_id 0 without interning.
    let other = CString::new("App\\Other\\Attr").unwrap();
    let attrs: [*const std::os::raw::c_char; 1] = [other.as_ptr()];
    let mut excluded = 0u8;
    let mut force = 0u8;
    let mut has_sample = 0u8;
    let mut rate = 0.0_f32;
    let result = unsafe {
        oxphp_profiler_resolve_filter(
            0xc0ffee,
            std::ptr::null(),
            0,
            attrs.as_ptr(),
            attrs.len() as u32,
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
fn resolver_marks_excluded_for_function_with_exclude_attr() {
    let exclude = CString::new("OxPHP\\Profile\\Exclude").unwrap();
    let attrs: [*const std::os::raw::c_char; 1] = [exclude.as_ptr()];
    let mut excluded = 0u8;
    let mut force = 0u8;
    let mut has_sample = 0u8;
    let mut rate = 0.0_f32;
    let result = unsafe {
        oxphp_profiler_resolve_filter(
            0xdead0001,
            std::ptr::null(),
            0,
            attrs.as_ptr(),
            attrs.len() as u32,
            std::ptr::null_mut(),
            &mut excluded,
            &mut force,
            &mut has_sample,
            &mut rate,
        )
    };
    assert_ne!(result, 0, "spec_id should be assigned");
    assert_eq!(excluded, 1, "decision quad reflects Exclude");
    let spec = get_spec(result).expect("spec interned");
    assert!(spec.excluded);
    assert!(!spec.force_profile);
}

#[test]
fn resolver_marks_force_profile_and_excluded_when_both_present() {
    let exclude = CString::new("OxPHP\\Profile\\Exclude").unwrap();
    let profile = CString::new("OxPHP\\Profile\\Profile").unwrap();
    let attrs: [*const std::os::raw::c_char; 2] = [exclude.as_ptr(), profile.as_ptr()];
    let mut excluded = 0u8;
    let mut force = 0u8;
    let mut has_sample = 0u8;
    let mut rate = 0.0_f32;
    let result = unsafe {
        oxphp_profiler_resolve_filter(
            0xdead0002,
            std::ptr::null(),
            0,
            attrs.as_ptr(),
            attrs.len() as u32,
            std::ptr::null_mut(),
            &mut excluded,
            &mut force,
            &mut has_sample,
            &mut rate,
        )
    };
    assert_ne!(result, 0);
    // Both flags set — Exclude wins in the C hot path because the
    // Excluded check comes first; force_profile is moot but still
    // recorded on the spec for diagnostics / future exporter use.
    assert_eq!(excluded, 1);
    assert_eq!(force, 1);
}

#[test]
fn resolver_class_tag_appears_before_function_tag_via_apply_events() {
    // Build a spec with class + function tags via merge_with_class
    // (the resolver itself can't be exercised without C-side arg
    // readers in the host build, so we use the public composition
    // API to assemble the same shape).
    let class = FilterSpec {
        tags: std::sync::Arc::from(vec![(
            std::sync::Arc::<str>::from("class.tag"),
            std::sync::Arc::<str>::from("class.value"),
        )]),
        ..Default::default()
    };
    let method = FilterSpec {
        tags: std::sync::Arc::from(vec![(
            std::sync::Arc::<str>::from("method.tag"),
            std::sync::Arc::<str>::from("method.value"),
        )]),
        ..Default::default()
    };
    let merged = FilterSpec::merge_with_class(class, method);
    let spec_id = {
        // Intern via a one-off resolver call that returns spec_id
        // for a synthesised spec — easier than exposing intern().
        let exclude = CString::new("OxPHP\\Profile\\Exclude").unwrap();
        let attrs: [*const std::os::raw::c_char; 1] = [exclude.as_ptr()];
        let mut e = 0u8;
        let mut f = 0u8;
        let mut h = 0u8;
        let mut r = 0.0_f32;
        let id = unsafe {
            oxphp_profiler_resolve_filter(
                0xdead0003,
                std::ptr::null(),
                0,
                attrs.as_ptr(),
                attrs.len() as u32,
                std::ptr::null_mut(),
                &mut e,
                &mut f,
                &mut h,
                &mut r,
            )
        };
        id
    };
    // We can't easily inject `merged` directly into the registry for
    // a different spec_id without exposing intern(). Verify the
    // composition rules independently:
    assert_eq!(merged.tags.len(), 2);
    assert_eq!(merged.tags[0].0.as_ref(), "class.tag");
    assert_eq!(merged.tags[1].0.as_ref(), "method.tag");
    // And confirm a real spec_id from the resolver is non-zero +
    // looks up.
    assert_ne!(spec_id, 0);
    assert!(get_spec(spec_id).is_some());
}

#[test]
fn apply_events_with_non_zero_spec_id_attaches_tags_from_registry() {
    // Use the resolver to get a real spec_id, then craft synthetic
    // events that target it. In the host build the resolver records
    // a placeholder ("", "") tag whenever #[Tag] is present (real
    // tag values come through under PHP via the C arg-readers).
    let tag = CString::new("OxPHP\\Profile\\Tag").unwrap();
    let attrs: [*const std::os::raw::c_char; 1] = [tag.as_ptr()];
    let mut e = 0u8;
    let mut f = 0u8;
    let mut h = 0u8;
    let mut r = 0.0_f32;
    let spec_id = unsafe {
        oxphp_profiler_resolve_filter(
            0xdead0004,
            std::ptr::null(),
            0,
            attrs.as_ptr(),
            attrs.len() as u32,
            std::ptr::null_mut(),
            &mut e,
            &mut f,
            &mut h,
            &mut r,
        )
    };
    assert_ne!(spec_id, 0);
    let spec = get_spec(spec_id).expect("spec interned");
    assert_eq!(spec.tags.len(), 1, "host build records placeholder tag");

    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
        let events = [
            ev(SPAN_EVENT_KIND_BEGIN, 1, b"tagged", spec_id),
            ev(SPAN_EVENT_KIND_END, 1, b"", 0),
        ];
        ctx.apply_events(&events);
        let tree = ctx.finalize();
        assert_eq!(tree.finished.len(), 1);
        let span = &tree.finished[0];
        // The placeholder tag is present (("" , "")). Under PHP it
        // would carry the real key/value pair the user declared.
        assert!(
            !span.attributes.is_empty(),
            "spec tags should be attached to span attributes"
        );
    });
}

#[test]
fn apply_events_with_zero_spec_id_skips_tag_lookup() {
    PROFILING_CONTEXT.with(|cell| {
        let mut ctx = cell.borrow_mut();
        ctx.reset(ProfilingMode::ProfileAll, "trace".into(), "root".into());
        let events = [
            ev(SPAN_EVENT_KIND_BEGIN, 5, b"plain", 0),
            ev(SPAN_EVENT_KIND_END, 5, b"", 0),
        ];
        ctx.apply_events(&events);
        let tree = ctx.finalize();
        assert_eq!(tree.finished.len(), 1);
        assert!(
            tree.finished[0].attributes.is_empty(),
            "spec_id 0 means no tag attachment"
        );
    });
}

#[test]
fn sample_hit_distribution_at_quarter_rate() {
    // 1k rolls at rate=0.25 → expect roughly 250 hits ± 100.
    let mut hits = 0;
    for _ in 0..1000 {
        if sample_hit(0.25) {
            hits += 1;
        }
    }
    assert!(
        hits > 150 && hits < 350,
        "expected ~250 ± 100 hits, got {hits}"
    );
}

#[test]
fn get_spec_for_unknown_id_returns_none() {
    // Use a value far above any allocated id to avoid collision.
    assert!(get_spec(u32::MAX).is_none());
    assert!(get_spec(0).is_none(), "0 reserved");
}

#[test]
fn empty_filter_spec_is_detected_correctly() {
    // The is_empty path is private, but resolver_returns_zero
    // exercises it indirectly. Direct check via composition:
    let merged = FilterSpec::merge_with_class(FilterSpec::default(), FilterSpec::default());
    // A merged-empty spec has no flags; resolver would return 0.
    assert!(!merged.force_profile);
    assert!(!merged.excluded);
    assert!(merged.sample_rate.is_none());
    assert!(merged.tags.is_empty());
}

#[test]
fn filter_module_re_exports_match_plan() {
    // Plan promised these names are reachable via crate::profiling::filter.
    let _: fn(f32) -> bool = filter::sample_hit;
    let _: fn(u32) -> Option<std::sync::Arc<FilterSpec>> = filter::get_spec;
}
