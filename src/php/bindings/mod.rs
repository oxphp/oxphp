//! PHP FFI bindings, split by ABI version.
//!
//! `common` contains every type whose layout is stable across the PHP
//! versions we currently target (8.4, 8.5). `v8_X` modules contain the
//! types whose layout differs, and exactly one is active per build,
//! selected by a cfg flag emitted by `build.rs`.
//!
//! Adding a new PHP version: copy the most recent `v8_X.rs`, apply the
//! upstream ABI delta, register a `#[cfg(php_v8_X)]` arm here, and
//! append a `(vernum_lo, vernum_hi, "v8_X")` row to `KNOWN_PHP_VERSIONS`
//! in `build.rs`. Removing an EOL'd version: delete the corresponding
//! `v8_X.rs` and matching cfg arms.

mod common;
pub use common::*;

#[cfg(php_v8_4)]
mod v8_4;
#[cfg(php_v8_4)]
pub use v8_4::*;

#[cfg(php_v8_5)]
mod v8_5;
#[cfg(php_v8_5)]
pub use v8_5::*;
