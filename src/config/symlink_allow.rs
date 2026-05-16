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
        canonical_root: &Path,
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
            let lookup_path = if Path::new(raw_entry).is_absolute() {
                PathBuf::from(raw_entry)
            } else {
                canonical_root.join(raw_entry)
            };
            let canonical = std::fs::canonicalize(&lookup_path).map_err(|e| -> BoxError {
                format!("SYMLINK_ALLOW_PATHS entry {raw_entry:?} canonicalize: {e}").into()
            })?;
            if !entries.contains(&canonical) {
                tracing::info!(
                    allow_path = %canonical.display(),
                    "SYMLINK_ALLOW_PATHS entry registered"
                );
                entries.push(canonical);
            }
        }
        Ok(Self { entries })
    }

    pub fn allows(&self, canonical_path: &Path) -> bool {
        self.entries
            .iter()
            .any(|e| canonical_path == e || canonical_path.starts_with(e))
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

    #[test]
    fn absolute_existing_path_is_registered() {
        let target = TempDir::new().unwrap();
        let target_canonical = std::fs::canonicalize(target.path()).unwrap();

        with_env(Some(target_canonical.to_str().unwrap()), || {
            let root = TempDir::new().unwrap();
            let canonical_root = std::fs::canonicalize(root.path()).unwrap();
            let list = SymlinkAllowList::from_env(&canonical_root).unwrap();
            assert!(!list.is_empty());
            assert!(list.allows(&target_canonical));
            assert!(list.allows(&target_canonical.join("nested/file.txt")));
        });
    }

    #[test]
    fn relative_entry_resolves_against_canonical_root() {
        let project = TempDir::new().unwrap();
        let project_canonical = std::fs::canonicalize(project.path()).unwrap();

        std::fs::create_dir(project_canonical.join("public")).unwrap();
        std::fs::create_dir(project_canonical.join("storage")).unwrap();
        let document_root = std::fs::canonicalize(project_canonical.join("public")).unwrap();
        let storage_canonical =
            std::fs::canonicalize(project_canonical.join("storage")).unwrap();

        with_env(Some("../storage"), || {
            let list = SymlinkAllowList::from_env(&document_root).unwrap();
            assert!(list.allows(&storage_canonical));
            assert!(list.allows(&storage_canonical.join("uploads/x.png")));
        });
    }

    #[test]
    fn missing_target_returns_err() {
        with_env(Some("/does/not/exist/anywhere/12345"), || {
            let root = TempDir::new().unwrap();
            let canonical_root = std::fs::canonicalize(root.path()).unwrap();
            let err = SymlinkAllowList::from_env(&canonical_root)
                .expect_err("missing target must error");
            let msg = err.to_string();
            assert!(
                msg.contains("/does/not/exist/anywhere/12345"),
                "error should name the offending entry, got: {msg}"
            );
            assert!(
                msg.contains("canonicalize"),
                "error should mention canonicalize, got: {msg}"
            );
        });
    }
}
