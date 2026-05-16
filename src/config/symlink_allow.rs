//! `SYMLINK_ALLOW_PATHS` parser and matcher.
//!
//! Parses the comma-separated env var into a vetted list of canonical
//! filesystem paths that symlinks inside `DOCUMENT_ROOT` are permitted to
//! resolve to. Default-empty preserves strict symlink-escape protection.

use std::path::{Path, PathBuf};

use crate::types::BoxError;

#[derive(Debug, Default)]
pub struct SymlinkAllowList {
    entries: Vec<PathBuf>,
}

impl SymlinkAllowList {
    pub fn from_env(canonical_root: &Path) -> Result<Self, BoxError> {
        Self::from_env_inner(canonical_root, current_username().as_deref())
    }

    fn from_env_inner(
        _canonical_root: &Path,
        _current_user: Option<&str>,
    ) -> Result<Self, BoxError> {
        let raw = std::env::var("SYMLINK_ALLOW_PATHS").unwrap_or_default();
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(Self::default());
        }
        let mut entries: Vec<PathBuf> = Vec::new();
        for part in trimmed.split(',') {
            let raw_entry = part.trim();
            if raw_entry.is_empty() {
                continue;
            }
            // Future tasks will resolve & validate `raw_entry` here.
            let _ = raw_entry;
        }
        Ok(Self { entries })
    }

    pub fn allows(&self, _canonical_path: &Path) -> bool {
        false
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

fn current_username() -> Option<String> {
    use std::ffi::CStr;
    use std::mem::MaybeUninit;

    let mut pwd: MaybeUninit<libc::passwd> = MaybeUninit::uninit();
    let mut buf = [0u8; 1024];
    let mut result: *mut libc::passwd = std::ptr::null_mut();

    let rc = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            pwd.as_mut_ptr(),
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };

    if rc != 0 || result.is_null() {
        return None;
    }

    unsafe {
        let pwd = pwd.assume_init();
        if pwd.pw_name.is_null() {
            return None;
        }
        CStr::from_ptr(pwd.pw_name).to_str().ok().map(String::from)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn with_env<F: FnOnce()>(value: Option<&str>, f: F) {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev = std::env::var("SYMLINK_ALLOW_PATHS").ok();
        match value {
            Some(v) => std::env::set_var("SYMLINK_ALLOW_PATHS", v),
            None => std::env::remove_var("SYMLINK_ALLOW_PATHS"),
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        match prev {
            Some(v) => std::env::set_var("SYMLINK_ALLOW_PATHS", v),
            None => std::env::remove_var("SYMLINK_ALLOW_PATHS"),
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    #[test]
    fn unset_env_returns_empty_list() {
        with_env(None, || {
            let root = TempDir::new().unwrap();
            let canonical_root = std::fs::canonicalize(root.path()).unwrap();
            let list = SymlinkAllowList::from_env(&canonical_root).unwrap();
            assert!(list.is_empty(), "expected empty list when env unset");
        });
    }

    #[test]
    fn empty_string_env_returns_empty_list() {
        with_env(Some(""), || {
            let root = TempDir::new().unwrap();
            let canonical_root = std::fs::canonicalize(root.path()).unwrap();
            let list = SymlinkAllowList::from_env(&canonical_root).unwrap();
            assert!(list.is_empty());
        });
    }

    #[test]
    fn whitespace_only_env_returns_empty_list() {
        with_env(Some("   ,  ,  "), || {
            let root = TempDir::new().unwrap();
            let canonical_root = std::fs::canonicalize(root.path()).unwrap();
            let list = SymlinkAllowList::from_env(&canonical_root).unwrap();
            assert!(list.is_empty());
        });
    }
}
