use std::borrow::Cow;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lru::LruCache;
use percent_encoding::percent_decode_str;

use crate::config::ServerConfig;
use crate::server::response::static_file::FileCache;

mod framework;
mod spa;
mod traditional;

#[cfg(test)]
mod tests;

use framework::FrameworkRouter;
use spa::SpaRouter;
use traditional::TraditionalRouter;

/// Routing mode dispatch. An enum rather than `Box<dyn ModeRouter>` so
/// each call site becomes a static `match` that rustc can inline, and
/// async methods return opaque `impl Future` instead of `BoxFuture` —
/// no heap allocation per request on the cache-miss path.
pub(crate) enum Mode {
    Traditional(TraditionalRouter),
    Framework(FrameworkRouter),
    Spa(SpaRouter),
}

impl Mode {
    async fn resolve_no_extension(&self, sanitized: &str, ctx: &ResolveCtx<'_>) -> RouteResult {
        match self {
            Mode::Traditional(r) => r.resolve_no_extension(sanitized, ctx).await,
            Mode::Framework(r) => r.resolve_no_extension(sanitized, ctx).await,
            Mode::Spa(r) => r.resolve_no_extension(ctx).await,
        }
    }

    async fn resolve_php(&self, sanitized: &str, ctx: &ResolveCtx<'_>) -> RouteResult {
        match self {
            Mode::Traditional(r) => r.resolve_php(sanitized, ctx).await,
            Mode::Framework(r) => r.resolve_php(sanitized, ctx).await,
            Mode::Spa(r) => r.resolve_php(sanitized, ctx).await,
        }
    }

    async fn resolve_static_miss(&self, sanitized: &str, ctx: &ResolveCtx<'_>) -> RouteResult {
        match self {
            Mode::Traditional(r) => r.resolve_static_miss(ctx).await,
            Mode::Framework(r) => r.resolve_static_miss(sanitized, ctx).await,
            Mode::Spa(_) => RouteResult::NotFound,
        }
    }
}

const ROUTE_CACHE_CAPACITY: usize = 10_000;

/// Result of route resolution.
#[derive(Debug, Clone)]
pub enum RouteResult {
    /// Execute a PHP script. `path_info` carries the extra path after the
    /// `.php` segment (PATH_INFO splitting). On the deny-fallback path
    /// `path_info` is always `None` — the original URI lives in
    /// `denied_meta.path` so it is not duplicated.
    /// `denied_meta` is `Some` only when this `Execute` is the PHP-script
    /// fallback for a `PHP_DENY_PATHS` match — drives `$_SERVER` enrichment.
    /// `Arc` keeps the variant 8-byte-tagged in the common (None) case and
    /// turns the rare-path clone into one atomic increment.
    Execute(
        PathBuf,
        Option<String>,
        Option<Arc<crate::config::DeniedMeta>>,
    ),
    /// Serve a static file.
    Serve(PathBuf),
    /// File not found.
    NotFound,
    /// Request was blocked by `PHP_DENY_PATHS` with an HTTP-status fallback.
    /// `ErrorPagesHandler` may substitute a body. Kept as a dedicated variant
    /// (not a generic `StatusCode`) so `connection.rs` can count denials
    /// without guessing about the source of the status code.
    Denied(u16),
}

/// Classification of a sanitized URI path performed once in the common layer
/// before delegating to a mode router.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UriKind {
    /// URI has no filename extension at all (`/foo`, `/api/users`, `/`).
    NoExtension,
    /// URI refers to a PHP script — either ends in `.php` or contains a
    /// `.php/` segment (PATH_INFO style). Matched case-insensitively.
    Php,
    /// URI has any other alphanumeric extension (`/style.css`, `/logo.png`).
    OtherExtension,
}

/// Context passed to every mode router on dispatch.
pub(crate) struct ResolveCtx<'a> {
    pub document_root: &'a Path,
    pub file_cache: &'a Arc<FileCache>,
    pub worker_route: Option<&'a RouteResult>,
    pub php_deny: Option<&'a crate::config::PhpDeny>,
}

/// Routing configuration with mode dispatch and caching layers.
pub struct RouteConfig {
    document_root: Arc<PathBuf>,
    canonical_root: PathBuf,
    mode: Mode,
    worker_route: Option<RouteResult>,
    php_deny: Option<crate::config::PhpDeny>,
    symlink_allow: crate::config::SymlinkAllowList,
    /// Cache of resolved routes keyed by URI path. `Mutex` rather than
    /// `RwLock` because `std::sync::RwLock` wraps `pthread_rwlock_t` on
    /// Linux and is ~2–3× slower than a futex-based `Mutex` in the
    /// uncontended case — which dominates the cache-hit hot path. Reads
    /// use `peek()` to skip LRU promotion so lock hold time stays O(1)
    /// with no linked-list splice. LRU ordering is only updated on
    /// insert; popular entries stay resident because they are re-inserted
    /// after any eviction. Values are `Arc<RouteResult>` so hits are a
    /// single atomic increment rather than a `PathBuf::clone()`.
    route_cache: Mutex<LruCache<String, Arc<RouteResult>>>,
}

impl RouteConfig {
    /// Create route config.
    ///
    /// Router selection is driven by `(worker_mode_enabled, entry_file extension)`:
    /// - worker mode → `TraditionalRouter`; the worker route fallback (set later
    ///   via [`set_worker_route`]) catches every unmatched request,
    /// - non-worker mode + `*.php` entry → `FrameworkRouter` (front controller),
    /// - non-worker mode + non-`.php` entry → `SpaRouter` (static fallback),
    /// - non-worker mode + no entry → `TraditionalRouter` (direct file mapping).
    ///
    /// Panics if the document root cannot be canonicalized, since symlink
    /// escape protection requires a valid, resolvable document root path.
    pub fn new(
        config: &ServerConfig,
        entry_file: Option<&Path>,
        worker_mode_enabled: bool,
    ) -> Self {
        let canonical_root = std::fs::canonicalize(&config.document_root).unwrap_or_else(|e| {
            panic!(
                "Fatal: cannot canonicalize document_root '{}': {}. \
                 Symlink escape protection requires a valid document root path.",
                config.document_root.display(),
                e
            );
        });

        let document_root = Arc::new(config.document_root.clone());

        let mode = if worker_mode_enabled {
            Mode::Traditional(TraditionalRouter::new(&document_root))
        } else {
            match entry_file.and_then(|p| {
                let ext = p.extension().and_then(|s| s.to_str())?;
                let name = p.file_name().and_then(|s| s.to_str())?;
                Some((ext.to_ascii_lowercase(), name))
            }) {
                None => Mode::Traditional(TraditionalRouter::new(&document_root)),
                Some((ext, name)) if ext == "php" => {
                    Mode::Framework(FrameworkRouter::new(&document_root, name))
                }
                Some((_, name)) => Mode::Spa(SpaRouter::new(&document_root, name)),
            }
        };

        let php_deny = crate::config::PhpDeny::from_env(&canonical_root, entry_file)
            .unwrap_or_else(|e| {
                panic!("Fatal: invalid PHP_DENY_* configuration: {e}");
            });

        let symlink_allow = crate::config::SymlinkAllowList::from_env(&canonical_root)
            .unwrap_or_else(|e| {
                panic!("Fatal: invalid SYMLINK_ALLOW_PATHS configuration: {e}");
            });

        Self {
            document_root,
            canonical_root,
            mode,
            worker_route: None,
            php_deny,
            symlink_allow,
            route_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(ROUTE_CACHE_CAPACITY).unwrap(),
            )),
        }
    }

    /// Returns the canonical document root.
    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    /// Returns the document root path.
    pub fn document_root(&self) -> &Path {
        &self.document_root
    }

    /// Returns a shared reference to the document root (cheap clone via Arc).
    pub fn document_root_arc(&self) -> Arc<PathBuf> {
        Arc::clone(&self.document_root)
    }

    /// Set the worker entry script for worker-mode routing. When set, all
    /// unmatched requests fall back to this PHP file before returning NotFound.
    pub fn set_worker_route(&mut self, path: PathBuf) {
        self.worker_route = Some(RouteResult::Execute(path, None, None));
    }

    /// Resolve a URI path to a route result using the file cache.
    ///
    /// Pipeline: dot-path block → route cache → decode → sanitize →
    /// well-known PHP block → classify → mode dispatch → symlink validation → cache.
    pub async fn resolve_request(
        &self,
        uri_path: &str,
        file_cache: &Arc<FileCache>,
    ) -> Arc<RouteResult> {
        // Block dot-paths before cache (keeps junk out of LRU). Fast-path:
        // byte scan lets clean URIs (`/api/users`, `/style.css`, `/`) skip
        // percent-decoding entirely. When markers are present we decode once
        // and hand the decoded string down the pipeline so the post-cache
        // decode step can reuse it.
        let pre_decoded: Option<String> = if has_dot_segment_markers(uri_path) {
            let decoded = match percent_decode_str(uri_path).decode_utf8() {
                Ok(s) => s,
                Err(_) => return Arc::new(RouteResult::NotFound),
            };
            if contains_blocked_dot_segment(&decoded) {
                return Arc::new(RouteResult::NotFound);
            }
            Some(decoded.into_owned())
        } else {
            None
        };

        // Fast path: route cache. `Mutex::lock()` + `peek()` — futex-backed
        // lock is cheaper uncontended than `RwLock::read()` on Linux, and
        // `peek()` keeps the critical section O(1) (no LRU promotion).
        {
            let cache = self.route_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(result) = cache.peek(uri_path) {
                return Arc::clone(result);
            }
        }

        // Decode URI — reuse the buffer produced during the dot-path screen
        // when available; otherwise decode now.
        let decoded = match pre_decoded {
            Some(s) => s,
            None => match percent_decode_str(uri_path).decode_utf8() {
                Ok(s) => s.into_owned(),
                Err(_) => return Arc::new(RouteResult::NotFound),
            },
        };

        // Sanitize path (strip `..`, `.`, empty segments). Most URIs are
        // already clean and yield a borrowed slice — no allocation.
        let sanitized_cow = sanitize_path(&decoded);
        let sanitized: &str = &sanitized_cow;

        // Compute has_php_component() once and share it across the
        // .well-known defence-in-depth check and URI classification.
        let has_php = has_php_component(sanitized);

        // Defense-in-depth: never execute PHP inside `.well-known/`
        if has_php && sanitized.starts_with(".well-known/") {
            let arc = Arc::new(RouteResult::NotFound);
            self.cache_put(uri_path, &arc);
            return arc;
        }

        // Classify once and dispatch
        let ctx = ResolveCtx {
            document_root: &self.document_root,
            file_cache,
            worker_route: self.worker_route.as_ref(),
            php_deny: self.php_deny.as_ref(),
        };

        let result = match classify_uri_with_php(sanitized, has_php) {
            UriKind::NoExtension => self.mode.resolve_no_extension(sanitized, &ctx).await,
            UriKind::Php => self.mode.resolve_php(sanitized, &ctx).await,
            UriKind::OtherExtension => {
                // Common disk check for non-.php extensions — done once here,
                // shared across all three modes via file_cache.
                let candidate = self.document_root.join(sanitized);
                if file_cache.is_file(&candidate.to_string_lossy()).await {
                    RouteResult::Serve(candidate)
                } else {
                    self.mode.resolve_static_miss(sanitized, &ctx).await
                }
            }
        };

        // Symlink-escape protection: the resolved path must live inside
        // the canonical document root. The worker file is an admin-configured
        // trusted path that may sit outside DOCUMENT_ROOT — skip it.
        let result = match &result {
            RouteResult::Serve(path) | RouteResult::Execute(path, _, _) => {
                let is_worker = self
                    .worker_route
                    .as_ref()
                    .is_some_and(|wr| matches!(wr, RouteResult::Execute(wf, _, _) if wf == path));
                if !is_worker && !self.validate_path(path, file_cache).await {
                    tracing::warn!(
                        path = %path.display(),
                        "Blocked request: resolved path escapes document root"
                    );
                    RouteResult::NotFound
                } else {
                    result
                }
            }
            RouteResult::NotFound | RouteResult::Denied(_) => result,
        };

        let arc = Arc::new(result);
        // Skip the route cache for `PHP_DENY_PATHS` results. Both `Denied`
        // and `Execute(_, _, Some(_))` are produced from attacker-controlled
        // URIs with effectively unbounded cardinality — caching them would
        // let an attacker spraying random `/uploads/{nonce}.php` evict hot
        // legitimate entries from the LRU. Re-running `resolve_php` on a
        // repeat denial costs only the `globset` byte scan; no disk I/O,
        // no syscall.
        let should_cache = !matches!(
            &*arc,
            RouteResult::Denied(_) | RouteResult::Execute(_, _, Some(_))
        );
        if should_cache {
            self.cache_put(uri_path, &arc);
        }
        arc
    }

    fn cache_put(&self, uri_path: &str, result: &Arc<RouteResult>) {
        let mut cache = self.route_cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.put(uri_path.to_string(), Arc::clone(result));
    }

    /// Check that a resolved path stays within the canonical document root
    /// or any explicitly allowed symlink target. Results are cached in the
    /// file cache to avoid repeated `realpath(3)`.
    async fn validate_path(&self, path: &Path, file_cache: &Arc<FileCache>) -> bool {
        let cache_key = path.to_string_lossy();

        if let Some(cached) = file_cache.get_canonical(&cache_key) {
            return match cached {
                Some(canonical_path) => self.is_path_within_allowed(&canonical_path),
                None => true, // file didn't exist at cache time; serve() will 404
            };
        }

        let result = tokio::fs::canonicalize(path).await.ok();
        let valid = match &result {
            Some(canonical_path) => self.is_path_within_allowed(canonical_path),
            None => true,
        };

        file_cache.insert_canonical(cache_key.into_owned(), result);
        valid
    }

    fn is_path_within_allowed(&self, canonical_path: &Path) -> bool {
        canonical_path.starts_with(&self.canonical_root)
            || self.symlink_allow.allows(canonical_path)
    }

    /// Allow the static-file serve path to consult the same allow-list
    /// without exposing the internal field.
    pub fn symlink_allow(&self) -> &crate::config::SymlinkAllowList {
        &self.symlink_allow
    }
}

/// Classify a sanitized URI path into one of three kinds.
///
/// Convenience wrapper that computes `has_php_component` internally. Callers
/// that already have the flag should use [`classify_uri_with_php`] instead
/// to avoid a redundant scan.
#[cfg(test)]
pub(crate) fn classify_uri(sanitized: &str) -> UriKind {
    classify_uri_with_php(sanitized, has_php_component(sanitized))
}

/// Classify a sanitized URI path into one of three kinds, reusing a
/// caller-computed `has_php_component` result.
pub(crate) fn classify_uri_with_php(sanitized: &str, has_php: bool) -> UriKind {
    // Any `.php` script component (end-of-string or followed by '/') wins.
    // Catches both `/about.php` and `/app.php/user/42` in one rule.
    if has_php {
        return UriKind::Php;
    }

    // Otherwise look at the last path segment's extension.
    let filename = match sanitized.rfind('/') {
        Some(pos) => &sanitized[pos + 1..],
        None => sanitized,
    };
    if filename.is_empty() {
        return UriKind::NoExtension;
    }
    let Some(dot) = filename.rfind('.') else {
        return UriKind::NoExtension;
    };
    if dot == 0 {
        // `.env`-like: dot-files are blocked upstream, but treat as no-ext
        // defensively so nothing slips through here.
        return UriKind::NoExtension;
    }
    let ext = &filename[dot + 1..];
    if !ext.is_empty() && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        UriKind::OtherExtension
    } else {
        UriKind::NoExtension
    }
}

/// Returns true if `sanitized` contains a `.php` script component —
/// either ends with `.php` or has `.php/` somewhere inside.
/// Case-insensitive to defend against `/admin.PHP` on case-insensitive FS.
///
/// Uses `memchr` to SIMD-scan for the leading `.` byte; each hit is
/// verified with an ASCII-case-insensitive byte compare against `php`
/// and a boundary check. On clean URIs with no `.` (`/`, `/api/users`)
/// memchr returns immediately on the first empty iteration.
pub(crate) fn has_php_component(sanitized: &str) -> bool {
    let bytes = sanitized.as_bytes();
    if bytes.len() < 4 {
        return false;
    }
    for dot in memchr::memchr_iter(b'.', bytes) {
        if dot + 3 >= bytes.len() {
            // Not enough room left for ".php" (needs 3 bytes after the dot).
            return false;
        }
        // ASCII case-insensitive compare: set bit 0x20 to force lowercase.
        if (bytes[dot + 1] | 0x20) == b'p'
            && (bytes[dot + 2] | 0x20) == b'h'
            && (bytes[dot + 3] | 0x20) == b'p'
        {
            let end = dot + 4;
            if end == bytes.len() || bytes[end] == b'/' {
                return true;
            }
        }
    }
    false
}

/// Byte-level screen: returns true when the URI is *provably clean* of any
/// dot-segment and percent-encoding, so the full decoded check can be skipped.
///
/// The vast majority of legitimate requests (`/api/users`, `/products/42`,
/// `/style.css`, `/`) trip none of these markers and skip the expensive
/// percent-decode + split path on the hot path.
#[inline]
fn has_dot_segment_markers(uri_path: &str) -> bool {
    let bytes = uri_path.as_bytes();
    if bytes.is_empty() {
        return false;
    }
    // Leading dot (e.g. `.env` passed without a slash prefix).
    if bytes[0] == b'.' {
        return true;
    }
    // Look for either `/.` (literal dot-segment start) or `%` (any percent-
    // encoded sequence — might hide a dot via %2e/%2E).
    for i in 0..bytes.len() {
        let b = bytes[i];
        if b == b'%' {
            return true;
        }
        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'.' {
            return true;
        }
    }
    false
}

/// Returns true if the already-decoded path contains a blocked dot-segment.
///
/// A dot-segment is any path component starting with `.` (e.g. `.git`, `.env`).
/// Exception: `.well-known` as the **first** segment with a non-empty sub-path
/// (`/.well-known/foo`) is allowed per RFC 8615. Bare `/.well-known` and
/// `/.well-known/` are blocked.
///
/// Operates on already-decoded input — callers own the single percent-decode
/// step so the decoded string can be reused down the pipeline.
fn contains_blocked_dot_segment(decoded: &str) -> bool {
    let mut segments = decoded.split('/').filter(|s| !s.is_empty());
    let mut is_first = true;

    while let Some(seg) = segments.next() {
        if !seg.starts_with('.') {
            is_first = false;
            continue;
        }

        if seg == ".well-known" && is_first {
            match segments.next() {
                Some(next) if !next.is_empty() && !next.starts_with('.') => {
                    is_first = false;
                    continue;
                }
                _ => return true,
            }
        }

        return true;
    }

    false
}

/// Remove `..`, `.` and empty segments from a path to prevent directory traversal.
///
/// Returns a borrowed slice (leading `/` stripped) when the input is already
/// clean — no `..`/`.` segments, no `//`, no trailing `/`. Most legitimate
/// URIs hit the borrow path with zero allocations; only URIs that actually
/// need rewriting pay for the `String` build.
fn sanitize_path(path: &str) -> Cow<'_, str> {
    if let Some(clean) = already_sanitized(path) {
        return Cow::Borrowed(clean);
    }
    let mut result = String::with_capacity(path.len());
    for segment in path.split('/') {
        if !segment.is_empty() && segment != ".." && segment != "." {
            if !result.is_empty() {
                result.push('/');
            }
            result.push_str(segment);
        }
    }
    Cow::Owned(result)
}

/// Fast byte-level check for `sanitize_path`'s borrow fast path. Returns
/// `Some(&path[1..])` when `path` is already in canonical form (exactly one
/// leading `/`, no `//`, no trailing `/`, no `.` or `..` segments). The
/// returned slice matches what `sanitize_path` would build via allocation.
#[inline]
fn already_sanitized(path: &str) -> Option<&str> {
    let bytes = path.as_bytes();
    if bytes.len() < 2 || bytes[0] != b'/' || bytes[bytes.len() - 1] == b'/' {
        return None;
    }
    let mut at_segment_start = true;
    let mut i = 1;
    while i < bytes.len() {
        let b = bytes[i];
        if b == b'/' {
            // `//` → empty segment
            if at_segment_start {
                return None;
            }
            at_segment_start = true;
            i += 1;
            continue;
        }
        if at_segment_start && b == b'.' {
            // Check for `.` or `..` as complete segment (delimited by '/' or end)
            match bytes.get(i + 1) {
                None | Some(&b'/') => return None,
                Some(&b'.') => match bytes.get(i + 2) {
                    None | Some(&b'/') => return None,
                    _ => {}
                },
                _ => {}
            }
        }
        at_segment_start = false;
        i += 1;
    }
    Some(&path[1..])
}
