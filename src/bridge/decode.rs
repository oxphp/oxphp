//! Shared lossy decoding for strings crossing the FFI from PHP.
//!
//! PHP strings are byte strings — an exception class name, message or
//! stacktrace may carry non-UTF-8 bytes (latin1 from a database, binary
//! payloads) or embedded NULs (an anonymous class name holds one). All three
//! cross the FFI length-delimited, and both exception-capture paths (the
//! `#[Trace]` decorator via `decorator::dispatch` and `oxphp_apm_error` via
//! `bridge::call`) must decode identically, so the logic lives here rather than
//! being reimplemented per path.

use std::borrow::Cow;
use std::os::raw::c_char;

/// Decode a nullable length-delimited byte string lossily. Honors `len` rather
/// than stopping at a NUL, so an embedded-NUL message or stacktrace is
/// preserved; invalid UTF-8 becomes U+FFFD rather than dropping the value.
///
/// # Safety
/// `p` must be null or point to at least `len` valid bytes.
pub(crate) unsafe fn bytes_lossy<'a>(p: *const c_char, len: usize) -> Option<Cow<'a, str>> {
    if p.is_null() || len == 0 {
        None
    } else {
        let bytes = std::slice::from_raw_parts(p as *const u8, len);
        Some(String::from_utf8_lossy(bytes))
    }
}
