#[cfg(feature = "php")]
pub mod ffi;

#[cfg(not(feature = "php"))]
pub mod mock;

pub mod call;
pub mod cancel;
pub(crate) mod decode;
pub mod storage;
pub mod types;

// Re-export the FFI module as `ffi` regardless of feature flag.
// Mock provides the same signatures for host testing.
#[cfg(not(feature = "php"))]
pub use mock as ffi;

/// The runtime-hook categories this process installed at PHP module startup,
/// in the order the extension declares them.
///
/// Empty means no hook is installed, which is the same answer for `RUNTIME_HOOKS`
/// unset, set to `0`, spelled with a typo in the variable name, or naming only
/// categories this build does not know — every one of those enables nothing.
/// It is also what a build without PHP reads, since nothing there installs hooks.
///
/// This reports what was *installed*, which is not the same as what is in
/// effect: where there are no fibers every hook delegates to the handler it
/// replaced, so a traditional-mode process reports the categories it swapped while behaving
/// natively throughout.
///
/// The extension parses `RUNTIME_HOOKS` and publishes the result; nothing here
/// reads the variable, so there is no second copy of that grammar to drift.
pub fn runtime_hooks() -> Vec<String> {
    // Safety: the pointer addresses a static buffer in the bridge that is
    // written once on the startup thread before any worker thread is spawned
    // and never mutated afterwards. It is never NULL.
    let csv = unsafe { std::ffi::CStr::from_ptr(ffi::oxphp_bridge_get_runtime_hooks()) };

    csv.to_string_lossy()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}
