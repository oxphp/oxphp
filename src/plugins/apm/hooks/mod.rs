//! APM hook infrastructure — registers internal PHP function hooks for
//! automatic span creation around database, HTTP client, cache, and I/O calls.
//!
//! ## Architecture
//!
//! 1. **Registration phase** (Rust, before PHP startup): `register_all()` calls
//!    each submodule's `register()` which calls `register_hook(class, func)`.
//!    This stores entries in the C bridge's pending hook list.
//!
//! 2. **Callback installation** (Rust, before PHP startup): `install_callbacks()`
//!    sets the before/after function pointers in the C bridge.
//!
//! 3. **Hook installation** (C, during MINIT): `oxphp_apm_install_registered_hooks()`
//!    looks up each pending function in Zend's tables and replaces its handler.
//!
//! 4. **Runtime** (C, during PHP execution): The wrapper calls before → original → after.
//!    The Rust callbacks create/close spans on the thread-local `SpanStack`.

pub mod curl;
pub mod file_io;
pub mod memcached;
pub mod mysqli;
pub mod pdo;
pub mod redis;

use std::cell::RefCell;
use std::time::Instant;

/// A frame pushed onto the thread-local stack by `before_callback` and
/// popped by `after_callback`. Carries state between the two calls.
#[derive(Debug)]
pub struct HookFrame {
    /// Span local ID returned by `SpanStack::push`.
    pub span_local_id: u32,
    /// Timestamp when the before callback fired (for precise timing).
    pub start: Instant,
}

thread_local! {
    /// Stack of active hook frames, mirroring the PHP call nesting.
    static HOOK_FRAMES: RefCell<Vec<HookFrame>> = const { RefCell::new(Vec::new()) };
}

/// Push a frame onto the thread-local hook frame stack.
pub fn push_frame(frame: HookFrame) {
    HOOK_FRAMES.with(|frames| frames.borrow_mut().push(frame));
}

/// Pop the most recent frame from the thread-local hook frame stack.
pub fn pop_frame() -> Option<HookFrame> {
    HOOK_FRAMES.with(|frames| frames.borrow_mut().pop())
}

// ---------------------------------------------------------------------------
// FFI bindings (only when compiling with PHP)
// ---------------------------------------------------------------------------

#[cfg(feature = "php")]
mod ffi {
    use std::os::raw::c_char;

    extern "C" {
        pub fn oxphp_apm_set_before(
            f: Option<
                unsafe extern "C" fn(*const c_char, *const c_char, u32, *mut std::ffi::c_void),
            >,
        );
        pub fn oxphp_apm_set_after(
            f: Option<
                unsafe extern "C" fn(
                    *const c_char,
                    *const c_char,
                    u32,
                    *mut std::ffi::c_void,
                    *mut std::ffi::c_void,
                ),
            >,
        );
        pub fn oxphp_apm_register_hook(class_name: *const c_char, func_name: *const c_char);
        pub fn oxphp_apm_hook_count_installed() -> i32;
        pub fn oxphp_apm_hook_count_approved() -> i32;
        pub fn oxphp_apm_unhook_all();
    }
}

// ---------------------------------------------------------------------------
// Registration (called from Rust before PHP startup)
// ---------------------------------------------------------------------------

/// Register a single internal PHP function for hooking.
///
/// `class_name` should be the PHP class name (e.g. "PDO") or empty string
/// for global functions. `func_name` is the method/function name.
///
/// This only records the intent — actual installation happens during MINIT.
#[cfg(feature = "php")]
pub fn register_hook(class_name: &str, func_name: &str) {
    use std::ffi::CString;
    let c_class = CString::new(class_name).unwrap_or_default();
    let c_func = CString::new(func_name).unwrap_or_default();
    unsafe {
        ffi::oxphp_apm_register_hook(c_class.as_ptr(), c_func.as_ptr());
    }
}

#[cfg(not(feature = "php"))]
pub fn register_hook(_class_name: &str, _func_name: &str) {
    // No-op on host without PHP
}

/// Register all hook targets across all submodules.
/// Returns the total number of functions registered for hooking.
pub fn register_all() -> usize {
    let mut count = 0;
    count += pdo::register();
    count += mysqli::register();
    count += curl::register();
    count += redis::register();
    count += memcached::register();
    count += file_io::register();
    count
}

/// Set the Rust before/after callbacks in the C bridge.
#[cfg(feature = "php")]
pub fn install_callbacks() {
    unsafe {
        ffi::oxphp_apm_set_before(Some(before_callback));
        ffi::oxphp_apm_set_after(Some(after_callback));
    }
}

#[cfg(not(feature = "php"))]
pub fn install_callbacks() {
    // No-op on host without PHP
}

/// Restore all hooks and clear callbacks.
#[cfg(feature = "php")]
pub fn unhook_all() {
    unsafe {
        ffi::oxphp_apm_unhook_all();
        ffi::oxphp_apm_set_before(None);
        ffi::oxphp_apm_set_after(None);
    }
}

#[cfg(not(feature = "php"))]
pub fn unhook_all() {}

/// Get count of installed hooks (for diagnostics).
#[cfg(feature = "php")]
pub fn hook_count() -> i32 {
    unsafe { ffi::oxphp_apm_hook_count_installed() }
}

#[cfg(not(feature = "php"))]
pub fn hook_count() -> i32 {
    0
}

/// Get count of approved hooks (global, available after MINIT).
#[cfg(feature = "php")]
pub fn approved_count() -> i32 {
    unsafe { ffi::oxphp_apm_hook_count_approved() }
}

#[cfg(not(feature = "php"))]
pub fn approved_count() -> i32 {
    0
}

// ---------------------------------------------------------------------------
// Rust callbacks invoked from C
// ---------------------------------------------------------------------------

/// Called before the original PHP internal function handler.
/// Creates a child span with the function name and pushes a HookFrame.
#[cfg(feature = "php")]
unsafe extern "C" fn before_callback(
    class_name: *const std::os::raw::c_char,
    func_name: *const std::os::raw::c_char,
    _argc: u32,
    _args: *mut std::ffi::c_void,
) {
    let cname = if class_name.is_null() {
        ""
    } else {
        unsafe { std::ffi::CStr::from_ptr(class_name) }
            .to_str()
            .unwrap_or("")
    };
    let fname = if func_name.is_null() {
        ""
    } else {
        unsafe { std::ffi::CStr::from_ptr(func_name) }
            .to_str()
            .unwrap_or("")
    };

    // Build span name: "ClassName::method" or just "function"
    let span_name = if cname.is_empty() {
        fname.to_string()
    } else {
        format!("{cname}::{fname}")
    };

    let start = Instant::now();

    // Push a span onto the APM span stack
    let local_id = super::spans::SPAN_STACK.with(|stack| {
        stack.borrow_mut().push(
            span_name,
            vec![("source".to_string(), "auto-hook".to_string())],
        )
    });

    push_frame(HookFrame {
        span_local_id: local_id,
        start,
    });
}

/// Called after the original PHP internal function handler.
/// Pops the HookFrame and closes the span.
#[cfg(feature = "php")]
unsafe extern "C" fn after_callback(
    _class_name: *const std::os::raw::c_char,
    _func_name: *const std::os::raw::c_char,
    _argc: u32,
    _args: *mut std::ffi::c_void,
    _return_value: *mut std::ffi::c_void,
) {
    if let Some(frame) = pop_frame() {
        super::spans::SPAN_STACK.with(|stack| {
            stack.borrow_mut().pop(frame.span_local_id);
        });
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_frame_push_pop() {
        // Clear any leftover state from other tests
        HOOK_FRAMES.with(|frames| frames.borrow_mut().clear());

        let frame1 = HookFrame {
            span_local_id: 1,
            start: Instant::now(),
        };
        let frame2 = HookFrame {
            span_local_id: 2,
            start: Instant::now(),
        };

        push_frame(frame1);
        push_frame(frame2);

        // Pop should return in LIFO order
        let popped = pop_frame().expect("should have frame");
        assert_eq!(popped.span_local_id, 2);

        let popped = pop_frame().expect("should have frame");
        assert_eq!(popped.span_local_id, 1);
    }

    #[test]
    fn test_hook_frame_empty_pop() {
        // Clear any leftover state
        HOOK_FRAMES.with(|frames| frames.borrow_mut().clear());

        assert!(pop_frame().is_none());
    }

    #[test]
    fn test_hook_frame_nested_push_pop() {
        HOOK_FRAMES.with(|frames| frames.borrow_mut().clear());

        // Simulate nested hooks: PDO::query -> file_get_contents
        push_frame(HookFrame {
            span_local_id: 10,
            start: Instant::now(),
        });
        push_frame(HookFrame {
            span_local_id: 20,
            start: Instant::now(),
        });

        // Inner pops first
        let inner = pop_frame().unwrap();
        assert_eq!(inner.span_local_id, 20);

        // Outer pops second
        let outer = pop_frame().unwrap();
        assert_eq!(outer.span_local_id, 10);

        // Stack is empty
        assert!(pop_frame().is_none());
    }

    #[test]
    fn test_register_all_returns_count() {
        let count = register_all();
        // All submodules should register their functions
        // PDO: 5, mysqli: 4, curl: 4, redis: 10, memcached: 5, file_io: 5 = 33
        assert!(
            count > 0,
            "register_all should register at least some hooks"
        );
        assert_eq!(count, 33);
    }

    #[test]
    fn test_register_hook_no_php() {
        // On host without PHP, this should be a no-op (not panic)
        register_hook("PDO", "query");
        register_hook("", "file_get_contents");
    }
}
