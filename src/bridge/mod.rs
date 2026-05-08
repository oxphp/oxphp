#[cfg(feature = "php")]
pub mod ffi;

#[cfg(not(feature = "php"))]
pub mod mock;

pub mod call;
pub mod cancel;
pub mod storage;
pub mod types;

// Re-export the FFI module as `ffi` regardless of feature flag.
// Mock provides the same signatures for host testing.
#[cfg(not(feature = "php"))]
pub use mock as ffi;
