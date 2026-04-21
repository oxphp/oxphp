//! FFI receiver for batched span events from the C-side profiler
//! observer (`ext/bridge/oxphp_bridge.c::g_prof`). Called from PHP
//! worker threads only; runs synchronously inside the observer
//! `begin` / `end` callbacks via the bridge's flush helper.
//!
//! The `OxSpanEvent` struct is `#[repr(C)]` and mirrors the C
//! `ox_span_event_t` definition byte-for-byte. The C side asserts
//! `_Static_assert(sizeof(ox_span_event_t) == 64)`; the Rust unit
//! test below mirrors that with `assert_eq!(size_of::<OxSpanEvent>(),
//! 64)`. **Field order is load-bearing** — changing this struct
//! requires changing the C header in lockstep.

#[cfg(feature = "php")]
use crate::profiling::PROFILING_CONTEXT;

use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::Arc;

/// Event kind tags. Must match `OXPHP_SPAN_EVENT_KIND_*` in
/// `ext/bridge/oxphp_bridge.h`.
pub const SPAN_EVENT_KIND_BEGIN: u8 = 1;
pub const SPAN_EVENT_KIND_END: u8 = 2;

/// Profiling-mode raw byte values. Mirror `OXPHP_PROFILING_MODE_*`
/// in `ext/bridge/oxphp_bridge.h` and the discriminants of
/// `crate::profiling::ProfilingMode`.
pub const PROFILING_MODE_OFF_RAW: u8 = 0;
pub const PROFILING_MODE_APM_ONLY_RAW: u8 = 1;
pub const PROFILING_MODE_PROFILE_ALL_RAW: u8 = 2;

/// Rust mirror of C `ox_span_event_t` (one cache line, 64 bytes).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct OxSpanEvent {
    pub kind: u8,
    pub reserved0: u8,
    pub name_len: u16,
    pub reserved1: u32,
    pub seq: u64,
    pub ts_ns: u64,
    pub cpu_ns: u64,
    pub mem: i64,
    pub mem_peak: i64,
    pub name_ptr: *const std::os::raw::c_char,
    pub reserved2: u64,
}

// SAFETY: OxSpanEvent is never shared across threads — the slice
// passed to `oxphp_profiler_flush_span_events` lives in C TLS and
// is read on the same worker thread that produced it.
unsafe impl Sync for OxSpanEvent {}

thread_local! {
    /// Process-lifetime intern table for BEGIN-event function names.
    /// A `HashSet<Arc<str>>` gives structural equality (lookup by
    /// `&str` via `Arc<str>: Borrow<str>`) so distinct names with
    /// colliding hashes never alias. PHP `zend_function` names are
    /// stable strings so the cache converges quickly (typically <10k
    /// entries for a real app); a 4096-entry soft cap protects
    /// against pathological generated-name workloads by flushing and
    /// repopulating rather than growing without bound.
    static NAME_INTERNER: RefCell<HashSet<Arc<str>>> =
        RefCell::new(HashSet::with_capacity(256));
}

/// Upper bound on the per-thread interner before a defensive flush.
const NAME_INTERNER_SOFT_CAP: usize = 4096;

/// Read `name_ptr` / `name_len` into an interned `Arc<str>`. Falls
/// back to a shared sentinel when the C-side arena ran out of room.
///
/// On cache hit this is a relaxed atomic increment — no allocation,
/// no UTF-8 validation copy. On miss we validate once (lossy) and
/// store the `Arc<str>` for subsequent calls.
pub(crate) fn read_name(ev: &OxSpanEvent) -> Arc<str> {
    if ev.name_ptr.is_null() || ev.name_len == 0 {
        return Arc::from("<unnamed>");
    }
    // SAFETY: contract of OxSpanEvent — name_ptr points to name_len
    // valid bytes inside the per-flush C arena, valid for the
    // duration of the flush call.
    let bytes =
        unsafe { std::slice::from_raw_parts(ev.name_ptr as *const u8, ev.name_len as usize) };
    let s = String::from_utf8_lossy(bytes);
    NAME_INTERNER.with(|cell| {
        let mut set = cell.borrow_mut();
        // `HashSet<Arc<str>>::get` uses `Arc<str>: Borrow<str>` so we
        // can look up structurally by `&str` without allocating.
        if let Some(existing) = set.get(s.as_ref()) {
            return Arc::clone(existing);
        }
        if set.len() >= NAME_INTERNER_SOFT_CAP {
            set.clear();
        }
        let arc: Arc<str> = Arc::from(s.as_ref());
        set.insert(Arc::clone(&arc));
        arc
    })
}

/// FFI entry point called by the C observer when its 256-deep ring
/// buffer fills or at RSHUTDOWN. `events` points into TLS-owned
/// memory and is valid only for the duration of this call.
///
/// # Safety
/// Callers must guarantee `events` is non-null when `count > 0` and
/// points to at least `count` initialised `OxSpanEvent` values for
/// the duration of the call.
#[cfg(feature = "php")]
#[no_mangle]
pub unsafe extern "C" fn oxphp_profiler_flush_span_events(events: *const OxSpanEvent, count: u32) {
    if events.is_null() || count == 0 {
        return;
    }
    let slice: &[OxSpanEvent] = std::slice::from_raw_parts(events, count as usize);
    PROFILING_CONTEXT.with(|cell| {
        cell.borrow_mut().apply_events(slice);
    });
}

// ─── Safe wrappers around the bridge entry points ────────────────
//
// When the `php` feature is off (host testing without libphp.so),
// the bridge symbols are unavailable, so we provide no-op stubs
// with the same signatures. Call sites in the executor / tests can
// invoke these unconditionally.

/// Set the per-thread profiling mode in the C bridge. Must be
/// called BEFORE `php_request_startup()` so the observer init
/// callback and the first `begin()` see the right mode.
pub fn set_profiling_mode(mode: crate::profiling::ProfilingMode) {
    let raw: u8 = match mode {
        crate::profiling::ProfilingMode::Off => PROFILING_MODE_OFF_RAW,
        crate::profiling::ProfilingMode::ApmOnly => PROFILING_MODE_APM_ONLY_RAW,
        crate::profiling::ProfilingMode::ProfileAll => PROFILING_MODE_PROFILE_ALL_RAW,
    };
    #[cfg(feature = "php")]
    unsafe {
        crate::php::bindings::oxphp_bridge_set_profiling_mode(raw);
    }
    #[cfg(not(feature = "php"))]
    {
        let _ = raw;
    }
}

/// Read the current per-thread bridge mode. Mainly for tests.
pub fn get_profiling_mode() -> u8 {
    #[cfg(feature = "php")]
    unsafe {
        crate::php::bindings::oxphp_bridge_get_profiling_mode()
    }
    #[cfg(not(feature = "php"))]
    {
        PROFILING_MODE_OFF_RAW
    }
}

/// Snapshot the per-thread open-span seq mirror from the C bridge.
/// Returns the depth (capped at `dst.len()`) or `255` if the real
/// depth overflowed the C-side 32-entry array. The heap hook is the
/// primary consumer.
pub fn snapshot_open_stack(dst: &mut [u32]) -> u8 {
    #[cfg(feature = "php")]
    unsafe {
        crate::php::bindings::oxphp_bridge_snapshot_open_stack(
            dst.as_mut_ptr(),
            dst.len().min(255) as u8,
        )
    }
    #[cfg(not(feature = "php"))]
    {
        let _ = dst;
        0
    }
}

/// Drain any partial ring buffer in the C bridge. Idempotent. Called
/// at RSHUTDOWN before `PROFILING_CONTEXT::finalize`.
pub fn profiler_rshutdown_flush() {
    #[cfg(feature = "php")]
    unsafe {
        crate::php::bindings::oxphp_bridge_profiler_rshutdown_flush();
    }
}

/// Set the per-thread paused flag in the C bridge. PHP's
/// `OxPHP\Profile\pause()` / `resume()` toggle this. When paused,
/// `oxphp_profiler_begin` early-returns; `end` still pops so open
/// spans close cleanly.
pub fn set_profiling_paused(paused: bool) {
    #[cfg(feature = "php")]
    unsafe {
        crate::php::bindings::oxphp_bridge_set_profiling_paused(u8::from(paused));
    }
    #[cfg(not(feature = "php"))]
    {
        let _ = paused;
    }
}

/// Read the per-thread paused flag. PHP's
/// `OxPHP\Profile\is_active()` consults this together with the
/// mode byte.
pub fn is_profiling_paused() -> bool {
    #[cfg(feature = "php")]
    unsafe {
        crate::php::bindings::oxphp_bridge_is_profiling_paused() != 0
    }
    #[cfg(not(feature = "php"))]
    {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ox_span_event_size_matches_c() {
        // Mirrors `_Static_assert` in oxphp_bridge.c — both sides must
        // agree on the byte layout.
        assert_eq!(std::mem::size_of::<OxSpanEvent>(), 64);
    }

    #[test]
    fn unnamed_event_falls_back() {
        let ev = OxSpanEvent {
            kind: SPAN_EVENT_KIND_BEGIN,
            reserved0: 0,
            name_len: 0,
            reserved1: 0,
            seq: 1,
            ts_ns: 100,
            cpu_ns: 10,
            mem: 0,
            mem_peak: 0,
            name_ptr: std::ptr::null(),
            reserved2: 0,
        };
        assert_eq!(read_name(&ev).as_ref(), "<unnamed>");
    }

    #[test]
    fn named_event_reads_back() {
        let name = b"App\\Service::run";
        let ev = OxSpanEvent {
            kind: SPAN_EVENT_KIND_BEGIN,
            reserved0: 0,
            name_len: name.len() as u16,
            reserved1: 0,
            seq: 1,
            ts_ns: 100,
            cpu_ns: 10,
            mem: 0,
            mem_peak: 0,
            name_ptr: name.as_ptr() as *const std::os::raw::c_char,
            reserved2: 0,
        };
        assert_eq!(read_name(&ev).as_ref(), "App\\Service::run");
    }

    #[test]
    fn interner_returns_shared_arc_on_repeat() {
        let name = b"Shared::name";
        let ev = OxSpanEvent {
            kind: SPAN_EVENT_KIND_BEGIN,
            reserved0: 0,
            name_len: name.len() as u16,
            reserved1: 0,
            seq: 1,
            ts_ns: 0,
            cpu_ns: 0,
            mem: 0,
            mem_peak: 0,
            name_ptr: name.as_ptr() as *const std::os::raw::c_char,
            reserved2: 0,
        };
        let a = read_name(&ev);
        let b = read_name(&ev);
        assert!(Arc::ptr_eq(&a, &b), "repeat read_name should share Arc");
    }

    fn read_name_for_test(bytes: &[u8]) -> Arc<str> {
        let ev = OxSpanEvent {
            kind: SPAN_EVENT_KIND_BEGIN,
            reserved0: 0,
            name_len: bytes.len() as u16,
            reserved1: 0,
            seq: 1,
            ts_ns: 0,
            cpu_ns: 0,
            mem: 0,
            mem_peak: 0,
            name_ptr: bytes.as_ptr() as *const std::os::raw::c_char,
            reserved2: 0,
        };
        read_name(&ev)
    }

    #[test]
    fn interner_distinguishes_colliding_hashes_by_bytes() {
        // Any two distinct byte sequences must get distinct Arc<str>.
        let a = read_name_for_test(b"foo");
        let b = read_name_for_test(b"bar");
        assert_ne!(a.as_ref(), b.as_ref());
        assert!(!Arc::ptr_eq(&a, &b));
    }
}
