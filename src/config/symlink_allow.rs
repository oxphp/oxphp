//! `SYMLINK_ALLOW_PATHS` parser and matcher.
//!
//! Parses the comma-separated env var into a vetted list of canonical
//! filesystem paths that symlinks inside `DOCUMENT_ROOT` are permitted to
//! resolve to. Default-empty preserves strict symlink-escape protection.

use std::path::{Path, PathBuf};

use crate::types::BoxError;

/// Paths whose exact canonical match is forbidden.
const BLACKLIST_EXACT: &[&str] = &[
    "/", "/etc", "/proc", "/sys", "/dev", "/var", "/home", "/tmp", "/root", "/usr",
];

/// Prefixes — a path whose canonical starts with `<prefix>/` is forbidden.
/// Note `/var` and `/home` are intentionally not here: they are exact-only
/// (admin may want `/var/www/...`, `/home/{current_user}/...`).
const BLACKLIST_PREFIXES: &[&str] = &["/etc", "/proc", "/sys", "/dev", "/tmp", "/root", "/usr"];

fn reject_blacklist(path: &Path, current_user: Option<&str>, raw_entry: &str) -> Result<(), BoxError> {
    let path_str = path.to_string_lossy();

    for exact in BLACKLIST_EXACT {
        if path_str == *exact {
            return Err(format!(
                "SYMLINK_ALLOW_PATHS entry {raw_entry:?} matches {path_str:?} which is in the exact blacklist"
            )
            .into());
        }
    }

    for prefix in BLACKLIST_PREFIXES {
        let prefix_slash = format!("{prefix}/");
        if path_str.starts_with(&prefix_slash) {
            return Err(format!(
                "SYMLINK_ALLOW_PATHS entry {raw_entry:?} matches {path_str:?} which lies under blacklisted prefix {prefix:?}"
            )
            .into());
        }
    }

    if let Some(rest) = path_str.strip_prefix("/home/") {
        let first_component = rest.split('/').next().unwrap_or("");
        if first_component.is_empty() {
            return Err(format!(
                "SYMLINK_ALLOW_PATHS entry {raw_entry:?} matches {path_str:?} which is /home itself (blacklist)"
            )
            .into());
        }
        let allowed_user = current_user.unwrap_or("");
        if allowed_user.is_empty() || first_component != allowed_user {
            return Err(format!(
                "SYMLINK_ALLOW_PATHS entry {raw_entry:?} matches {path_str:?} under another user's home directory (blacklist)"
            )
            .into());
        }
    }

    Ok(())
}

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
        current_user: Option<&str>,
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
            let raw_path = Path::new(raw_entry);
            let lookup_path = if raw_path.is_absolute() {
                // Pre-canonicalize blacklist check on the admin-typed path.
                // Required because on macOS `/etc`, `/tmp`, `/var` etc. are
                // symlinks into `/private/...`, so a post-canonicalize check
                // alone would let `/tmp` slip through.
                reject_blacklist(raw_path, current_user, raw_entry)?;
                PathBuf::from(raw_entry)
            } else {
                canonical_root.join(raw_entry)
            };
            let canonical = std::fs::canonicalize(&lookup_path).map_err(|e| -> BoxError {
                format!("SYMLINK_ALLOW_PATHS entry {raw_entry:?} canonicalize: {e}").into()
            })?;
            // Defense-in-depth: catch symlink-target escapes (relative entry
            // that lands in a blacklisted dir, or an absolute that points
            // via symlink to a blacklisted canonical form).
            reject_blacklist(&canonical, current_user, raw_entry)?;
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

    #[test]
    fn exact_blacklist_rejected() {
        let cases = ["/etc", "/var", "/usr", "/tmp"];
        for target in cases {
            if !std::path::Path::new(target).is_dir() {
                continue;
            }
            with_env(Some(target), || {
                let root = TempDir::new().unwrap();
                let canonical_root = std::fs::canonicalize(root.path()).unwrap();
                let err = SymlinkAllowList::from_env(&canonical_root)
                    .expect_err(&format!("{target} must be rejected"));
                let msg = err.to_string();
                assert!(
                    msg.contains("blacklist"),
                    "error should mention blacklist for {target}, got: {msg}"
                );
            });
        }
    }
}
