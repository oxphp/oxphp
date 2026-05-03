//! PHP execution deny-list for Traditional routing mode.
//!
//! Matches sanitized URI paths against a `GlobSet` built from `PHP_DENY_DIRS`
//! and produces a `DenyFallback` (HTTP status or PHP script redirect) when
//! a request would otherwise execute PHP inside a denied directory.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use globset::{GlobSet, GlobSetBuilder};

use crate::types::BoxError;

thread_local! {
    /// Reused scratch buffer for `GlobSet::matches_into`. Amortizes the
    /// per-request allocation on the hit path — denied requests now bypass
    /// the route cache (cardinality DoS protection), so this buffer is
    /// touched on every denial rather than once per unique URI.
    static MATCH_BUF: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

/// What to return when a request matches `PHP_DENY_DIRS`.
#[derive(Debug, Clone)]
pub enum DenyFallback {
    /// Respond with a bare HTTP status (`ErrorPagesHandler` may substitute a body).
    Status(u16),
    /// Execute a PHP script instead of the requested one.
    Script {
        /// Canonical absolute path to the fallback script.
        path: PathBuf,
        /// Precomputed URI-form of `path` relative to the canonical
        /// `DOCUMENT_ROOT` (always leading `/`, e.g. `/_security/denied.php`).
        /// Stored here so the SAPI can populate `SCRIPT_NAME` without
        /// re-computing it — and, crucially, without needing access to the
        /// canonical root at request time (the raw `DOCUMENT_ROOT` and the
        /// canonical one can differ under symlinks, e.g. `/tmp` → `/private/tmp`).
        uri: String,
    },
}

/// Metadata attached to `RouteResult::Execute` on the deny-fallback path.
/// Drives `$_SERVER` enrichment inside the SAPI.
#[derive(Debug, Clone)]
pub struct DeniedMeta {
    /// Original sanitized URI (no leading `/`).
    pub path: String,
    /// Matched glob pattern from `PHP_DENY_DIRS`.
    pub pattern: String,
    /// URI-form of the fallback script (`/_security/denied.php`). Precomputed
    /// at config time from the canonical root — avoids a `strip_prefix` at
    /// request time that can silently fall back to the attacker URI when
    /// raw and canonical document roots differ.
    pub fallback_script_uri: String,
}

/// Compiled deny-list + fallback strategy.
#[derive(Debug)]
pub struct PhpDeny {
    matcher: GlobSet,
    patterns: Vec<String>,
    fallback: DenyFallback,
}

impl PhpDeny {
    /// Parse from environment. Returns `Ok(None)` when `PHP_DENY_DIRS` is unset
    /// or empty. Returns `Err` for malformed input. Emits a warn-and-disable
    /// path when an `entry_file` is configured — front-controller, SPA, and
    /// worker modes route every request through one trusted script, so
    /// arbitrary `.php` files in denied dirs cannot be invoked directly.
    pub fn from_env(
        document_root: &Path,
        entry_file: Option<&Path>,
    ) -> Result<Option<Self>, BoxError> {
        let raw = std::env::var("PHP_DENY_DIRS").unwrap_or_default();
        let raw = raw.trim();
        if raw.is_empty() {
            return Ok(None);
        }

        if entry_file.is_some() {
            tracing::warn!(
                "PHP_DENY_DIRS is set but ENTRY_FILE is also set — feature is direct-mapping-only, ignoring PHP_DENY_DIRS"
            );
            return Ok(None);
        }

        let patterns: Vec<String> = raw
            .split(',')
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .map(normalize_pattern)
            .collect();

        if patterns.is_empty() {
            return Ok(None);
        }

        let mut builder = GlobSetBuilder::new();
        for p in &patterns {
            let glob = globset::GlobBuilder::new(p)
                .literal_separator(true)
                .build()
                .map_err(|e| -> BoxError { format!("PHP_DENY_DIRS pattern {p:?}: {e}").into() })?;
            builder.add(glob);
        }
        let matcher = builder
            .build()
            .map_err(|e| -> BoxError { format!("PHP_DENY_DIRS build: {e}").into() })?;

        let fallback_raw = std::env::var("PHP_DENY_FALLBACK").unwrap_or_else(|_| "404".to_string());
        let fallback = parse_fallback(&fallback_raw, document_root, &matcher, &patterns)?;

        Ok(Some(Self {
            matcher,
            patterns,
            fallback,
        }))
    }

    /// If `sanitized` matches any pattern, return the first matching pattern.
    pub fn matches<'a>(&'a self, sanitized: &str) -> Option<&'a str> {
        // Fast path: `is_match` is a pure boolean check (no Vec allocation).
        // Only pay for the indices vec on a hit, which is the rare path.
        if !self.matcher.is_match(sanitized) {
            return None;
        }
        // Reuse a thread-local scratch Vec so the hit path stays alloc-free
        // after warmup. Lifetime juggling: extract the index inside the
        // closure, then look up the pattern outside so the returned `&'a str`
        // borrows from `self.patterns` (lifetime `'a`), not from the RefMut.
        let idx = MATCH_BUF.with(|buf| {
            let mut buf = buf.borrow_mut();
            self.matcher.matches_into(sanitized, &mut buf);
            buf.first().copied()
        });
        idx.map(|i| self.patterns[i].as_str())
    }

    pub fn fallback(&self) -> &DenyFallback {
        &self.fallback
    }
}

/// Strip the leading `/` so patterns match sanitized URIs (which have it stripped).
fn normalize_pattern(p: &str) -> String {
    p.strip_prefix('/').unwrap_or(p).to_string()
}

fn parse_fallback(
    raw: &str,
    document_root: &Path,
    matcher: &GlobSet,
    patterns: &[String],
) -> Result<DenyFallback, BoxError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("PHP_DENY_FALLBACK is empty (default is '404')".into());
    }

    // Status code?
    if let Ok(code) = raw.parse::<u16>() {
        if !(400..=599).contains(&code) {
            return Err(
                format!("PHP_DENY_FALLBACK status {code} is out of range 400..=599").into(),
            );
        }
        return Ok(DenyFallback::Status(code));
    }

    // Script path?
    if !raw.starts_with('/') {
        return Err(format!(
            "PHP_DENY_FALLBACK {raw:?} is neither a 4xx/5xx status nor a URI path starting with '/'"
        )
        .into());
    }
    let rel = raw.trim_start_matches('/');
    let fs_path = document_root.join(rel);

    let canonical = std::fs::canonicalize(&fs_path).map_err(|e| -> BoxError {
        format!(
            "PHP_DENY_FALLBACK script {} does not exist or is unreadable: {e}",
            fs_path.display()
        )
        .into()
    })?;

    let canonical_root = std::fs::canonicalize(document_root).map_err(|e| -> BoxError {
        format!(
            "DOCUMENT_ROOT {} canonicalize: {e}",
            document_root.display()
        )
        .into()
    })?;
    if !canonical.starts_with(&canonical_root) {
        return Err(format!(
            "PHP_DENY_FALLBACK script {} escapes DOCUMENT_ROOT",
            canonical.display()
        )
        .into());
    }

    // Anti-loop: fallback script itself must not match PHP_DENY_DIRS.
    let fallback_rel = canonical
        .strip_prefix(&canonical_root)
        .unwrap_or(&canonical)
        .to_string_lossy()
        .into_owned();
    if matcher.is_match(&fallback_rel) {
        let matched_idx = matcher.matches(&fallback_rel);
        let p = matched_idx
            .first()
            .map(|i| patterns[*i].as_str())
            .unwrap_or("?");
        return Err(format!(
            "PHP_DENY_FALLBACK script {fallback_rel} would be denied by its own rules (matches pattern {p:?}) — loop avoided"
        )
        .into());
    }

    // URI-form for `$_SERVER['SCRIPT_NAME']` — single leading `/`.
    let uri = format!("/{}", fallback_rel.trim_start_matches('/'));

    Ok(DenyFallback::Script {
        path: canonical,
        uri,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn deny_with(patterns: &str) -> PhpDeny {
        let patterns: Vec<String> = patterns
            .split(',')
            .map(|p| p.trim())
            .filter(|p| !p.is_empty())
            .map(normalize_pattern)
            .collect();
        let mut builder = GlobSetBuilder::new();
        for p in &patterns {
            let glob = globset::GlobBuilder::new(p)
                .literal_separator(true)
                .build()
                .unwrap();
            builder.add(glob);
        }
        PhpDeny {
            matcher: builder.build().unwrap(),
            patterns,
            fallback: DenyFallback::Status(404),
        }
    }

    #[test]
    fn matches_direct_php_in_deny_dir() {
        let d = deny_with("/uploads/**");
        assert_eq!(d.matches("uploads/shell.php"), Some("uploads/**"));
    }

    #[test]
    fn matches_nested_php_in_deny_dir() {
        let d = deny_with("/uploads/**");
        assert_eq!(
            d.matches("uploads/deep/nested/shell.php"),
            Some("uploads/**")
        );
    }

    #[test]
    fn matches_path_info_style() {
        let d = deny_with("/uploads/**");
        assert_eq!(d.matches("uploads/shell.php/extra"), Some("uploads/**"));
    }

    #[test]
    fn does_not_match_sibling_root_file() {
        let d = deny_with("/uploads/**");
        assert_eq!(d.matches("uploads.php"), None);
    }

    #[test]
    fn single_star_does_not_cross_slash() {
        let d = deny_with("/files/*.php");
        assert_eq!(d.matches("files/a.php"), Some("files/*.php"));
        assert_eq!(d.matches("files/sub/a.php"), None);
    }

    #[test]
    fn multiple_patterns_match_independently() {
        let d = deny_with("/uploads/**,/cache/**");
        assert_eq!(d.matches("uploads/x.php"), Some("uploads/**"));
        assert_eq!(d.matches("cache/y.php"), Some("cache/**"));
        assert_eq!(d.matches("public/z.php"), None);
    }

    #[test]
    fn leading_slash_optional() {
        let with = deny_with("/uploads/**");
        let without = deny_with("uploads/**");
        assert_eq!(with.matches("uploads/x.php"), Some("uploads/**"));
        assert_eq!(without.matches("uploads/x.php"), Some("uploads/**"));
    }

    #[test]
    fn no_match_returns_none() {
        let d = deny_with("/uploads/**");
        assert_eq!(d.matches("api/users"), None);
        assert_eq!(d.matches(""), None);
    }

    use std::sync::Mutex;
    use tempfile::TempDir;

    // Mutex serializes tests that touch process-global env vars.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn with_env<F: FnOnce()>(vars: &[(&str, Option<&str>)], f: F) {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev: Vec<(String, Option<String>)> = vars
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in vars {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        for (k, prev_val) in prev {
            match prev_val {
                Some(v) => std::env::set_var(&k, v),
                None => std::env::remove_var(&k),
            }
        }
        if let Err(e) = result {
            std::panic::resume_unwind(e);
        }
    }

    fn make_tempdir_with_file(subpath: &str) -> TempDir {
        let dir = TempDir::new().unwrap();
        let full = dir.path().join(subpath);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, b"<?php echo 'denied';\n").unwrap();
        dir
    }

    #[test]
    fn env_unset_returns_none() {
        with_env(
            &[("PHP_DENY_DIRS", None), ("PHP_DENY_FALLBACK", None)],
            || {
                let dir = TempDir::new().unwrap();
                let deny = PhpDeny::from_env(dir.path(), None).unwrap();
                assert!(deny.is_none());
            },
        );
    }

    #[test]
    fn env_empty_returns_none() {
        with_env(
            &[("PHP_DENY_DIRS", Some("")), ("PHP_DENY_FALLBACK", None)],
            || {
                let dir = TempDir::new().unwrap();
                let deny = PhpDeny::from_env(dir.path(), None).unwrap();
                assert!(deny.is_none());
            },
        );
    }

    #[test]
    fn env_with_entry_file_warns_and_disables() {
        with_env(
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", None),
            ],
            || {
                let dir = TempDir::new().unwrap();
                let entry = Path::new("/var/www/html/public/index.php");
                let deny = PhpDeny::from_env(dir.path(), Some(entry)).unwrap();
                assert!(deny.is_none());
            },
        );
    }

    #[test]
    fn env_default_fallback_is_404_status() {
        with_env(
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", None),
            ],
            || {
                let dir = TempDir::new().unwrap();
                let deny = PhpDeny::from_env(dir.path(), None).unwrap().unwrap();
                assert!(matches!(deny.fallback(), DenyFallback::Status(404)));
            },
        );
    }

    #[test]
    fn env_fallback_status_403() {
        with_env(
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", Some("403")),
            ],
            || {
                let dir = TempDir::new().unwrap();
                let deny = PhpDeny::from_env(dir.path(), None).unwrap().unwrap();
                assert!(matches!(deny.fallback(), DenyFallback::Status(403)));
            },
        );
    }

    #[test]
    fn env_fallback_status_out_of_range_errors() {
        with_env(
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", Some("399")),
            ],
            || {
                let dir = TempDir::new().unwrap();
                assert!(PhpDeny::from_env(dir.path(), None).is_err());
            },
        );
        with_env(
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", Some("600")),
            ],
            || {
                let dir = TempDir::new().unwrap();
                assert!(PhpDeny::from_env(dir.path(), None).is_err());
            },
        );
    }

    #[test]
    fn env_fallback_script_ok() {
        let dir = make_tempdir_with_file("_security/denied.php");
        let doc_root = dir.path().to_string_lossy().into_owned();
        with_env(
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", Some("/_security/denied.php")),
            ],
            || {
                let deny = PhpDeny::from_env(Path::new(&doc_root), None)
                    .unwrap()
                    .unwrap();
                match deny.fallback() {
                    DenyFallback::Script { uri, .. } => {
                        assert_eq!(uri, "/_security/denied.php");
                    }
                    other => panic!("expected Script fallback, got {other:?}"),
                }
            },
        );
    }

    #[test]
    fn env_fallback_script_missing_errors() {
        with_env(
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", Some("/does/not/exist.php")),
            ],
            || {
                let dir = TempDir::new().unwrap();
                assert!(PhpDeny::from_env(dir.path(), None).is_err());
            },
        );
    }

    #[test]
    fn env_fallback_script_in_denied_dir_errors() {
        let dir = make_tempdir_with_file("uploads/denied.php");
        let doc_root = dir.path().to_string_lossy().into_owned();
        with_env(
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", Some("/uploads/denied.php")),
            ],
            || {
                let err = PhpDeny::from_env(Path::new(&doc_root), None).unwrap_err();
                let msg = err.to_string();
                assert!(msg.contains("loop avoided"), "message was: {msg}");
            },
        );
    }

    #[test]
    fn env_invalid_glob_errors() {
        with_env(
            &[
                // Unclosed character class → glob parse error
                ("PHP_DENY_DIRS", Some("/uploads/[abc")),
                ("PHP_DENY_FALLBACK", None),
            ],
            || {
                let dir = TempDir::new().unwrap();
                assert!(PhpDeny::from_env(dir.path(), None).is_err());
            },
        );
    }

    #[test]
    fn env_malformed_fallback_errors() {
        with_env(
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", Some("banana")),
            ],
            || {
                let dir = TempDir::new().unwrap();
                assert!(PhpDeny::from_env(dir.path(), None).is_err());
            },
        );
    }
}
