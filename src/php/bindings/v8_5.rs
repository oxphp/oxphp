//! PHP 8.5 SAPI types — placeholder.
//!
//! Exists so `cargo fmt` resolves the cfg-gated `mod v8_5;` reference
//! in `mod.rs` without erroring out. The 8.5 SAPI struct + enum are
//! filled in by a follow-up commit on this branch.
//!
//! No `php_v8_5` cfg is active yet (`build.rs` still selects only
//! `php_v8_4`), so this file is not actually compiled in any current
//! build.

#![allow(non_camel_case_types)]
