//! SharedError enum + thread-local LAST_ERROR + FFI accessor.
//!
//! Spec: .internal/technical-docs/en/features/shared/05-exceptions.md

use std::cell::RefCell;
use std::os::raw::{c_char, c_int};

thread_local! {
    static LAST_ERROR: RefCell<String> = const { RefCell::new(String::new()) };
}

/// Rust-side error taxonomy. Each variant maps to one FFI status code
/// and one `Shared\*Exception` class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SharedError {
    /// Generic — message in LAST_ERROR.
    Generic,
    StaleHandle,
    Type,
    CapacityExceeded,
    Poisoned,
    Closed,
    Timeout,
    Deadlock,
    Cycle,
    Uninitialized,
    /// Panic caught at FFI boundary (Rust bug).
    Panicked,
}

impl SharedError {
    /// FFI status code per 05-exceptions.md §FFI status code → exception mapping.
    pub fn code(&self) -> i32 {
        match self {
            Self::Generic => -1,
            Self::StaleHandle => -2,
            Self::Type => -3,
            Self::CapacityExceeded => -4,
            Self::Poisoned => -5,
            Self::Closed => -6,
            Self::Timeout => -7,
            Self::Deadlock => -8,
            Self::Cycle => -9,
            Self::Uninitialized => -10,
            Self::Panicked => -99,
        }
    }
}

impl std::fmt::Display for SharedError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Generic => write!(f, "internal error"),
            Self::StaleHandle => write!(f, "shared entry was evicted"),
            Self::Type => write!(f, "type error"),
            Self::CapacityExceeded => write!(f, "capacity exceeded"),
            Self::Poisoned => write!(f, "poisoned by prior panic"),
            Self::Closed => write!(f, "closed"),
            Self::Timeout => write!(f, "timed out"),
            Self::Deadlock => write!(f, "deadlock detected"),
            Self::Cycle => write!(f, "cycle would form"),
            Self::Uninitialized => write!(f, "uninitialised wrapper"),
            Self::Panicked => write!(f, "internal: Rust panic at FFI boundary"),
        }
    }
}

pub fn set_last_error(msg: impl Into<String>) {
    LAST_ERROR.with(|e| {
        let mut s = e.borrow_mut();
        s.clear();
        s.push_str(&msg.into());
    });
}

pub fn clear_last_error() {
    LAST_ERROR.with(|e| e.borrow_mut().clear());
}

/// Copy the thread-local last-error into a caller-provided buffer.
/// Returns the message length (full byte length even if truncated).
/// If `buflen == 0` or `buf.is_null()`, returns the length without writing.
///
/// # Safety
///
/// `buf` must be valid for writes of `buflen` bytes and aligned for `c_char`.
/// Must be called from the same thread that set the error.
#[no_mangle]
pub unsafe extern "C" fn oxphp_shared_last_error(buf: *mut c_char, buflen: usize) -> usize {
    LAST_ERROR.with(|e| {
        let s = e.borrow();
        let bytes = s.as_bytes();
        let needed = bytes.len();
        if buf.is_null() || buflen == 0 {
            return needed;
        }
        let copy_len = needed.min(buflen.saturating_sub(1));
        // SAFETY: caller guarantees buf is valid for buflen bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, buf, copy_len);
            *buf.add(copy_len) = 0;
        }
        needed
    })
}

/// FFI wrapper that runs `body` inside `catch_unwind` and translates
/// errors into status codes per SharedError::code.
pub fn ffi_entry<F>(body: F) -> c_int
where
    F: FnOnce() -> Result<(), SharedError> + std::panic::UnwindSafe,
{
    match std::panic::catch_unwind(body) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => {
            if LAST_ERROR.with(|c| c.borrow().is_empty()) {
                set_last_error(e.to_string());
            }
            e.code()
        }
        Err(_) => {
            set_last_error("Rust panic at FFI boundary");
            SharedError::Panicked.code()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_match_spec() {
        assert_eq!(SharedError::Generic.code(), -1);
        assert_eq!(SharedError::StaleHandle.code(), -2);
        assert_eq!(SharedError::Type.code(), -3);
        assert_eq!(SharedError::CapacityExceeded.code(), -4);
        assert_eq!(SharedError::Poisoned.code(), -5);
        assert_eq!(SharedError::Closed.code(), -6);
        assert_eq!(SharedError::Timeout.code(), -7);
        assert_eq!(SharedError::Deadlock.code(), -8);
        assert_eq!(SharedError::Cycle.code(), -9);
        assert_eq!(SharedError::Uninitialized.code(), -10);
        assert_eq!(SharedError::Panicked.code(), -99);
    }

    #[test]
    fn last_error_round_trip() {
        clear_last_error();
        set_last_error("hello");
        let mut buf = [0u8; 32];
        let needed = unsafe { oxphp_shared_last_error(buf.as_mut_ptr() as *mut c_char, buf.len()) };
        assert_eq!(needed, 5);
        let s = std::ffi::CStr::from_bytes_until_nul(&buf).unwrap();
        assert_eq!(s.to_str().unwrap(), "hello");
    }

    #[test]
    fn last_error_truncates_safely() {
        clear_last_error();
        set_last_error("abcdefghij");
        let mut buf = [0u8; 5]; // room for 4 bytes + NUL
        let needed = unsafe { oxphp_shared_last_error(buf.as_mut_ptr() as *mut c_char, buf.len()) };
        assert_eq!(needed, 10);
        let s = std::ffi::CStr::from_bytes_until_nul(&buf).unwrap();
        assert_eq!(s.to_str().unwrap(), "abcd");
    }

    #[test]
    fn last_error_nullable_buf() {
        clear_last_error();
        set_last_error("probe");
        let needed = unsafe { oxphp_shared_last_error(std::ptr::null_mut(), 0) };
        assert_eq!(needed, 5);
    }

    #[test]
    fn ffi_entry_catches_panic() {
        let rc = ffi_entry(|| {
            panic!("boom");
        });
        assert_eq!(rc, -99);
    }

    #[test]
    fn ffi_entry_returns_error_code() {
        let rc = ffi_entry(|| Err(SharedError::StaleHandle));
        assert_eq!(rc, -2);
    }

    #[test]
    fn ffi_entry_happy_path() {
        let rc = ffi_entry(|| Ok(()));
        assert_eq!(rc, 0);
    }
}
