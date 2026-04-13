use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};

use futures_util::future::BoxFuture;
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

const ROUTE_CACHE_CAPACITY: usize = 10_000;

/// Result of route resolution.
#[derive(Debug, Clone)]
pub enum RouteResult {
    /// Execute a PHP script. Optional `path_info` carries the extra path
    /// after the `.php` segment (e.g. `/user/42` for `/app.php/user/42`),
    /// or the full original URI when a mode rewrites everything to a single
    /// front controller (Framework mode).
    Execute(PathBuf, Option<String>),
    /// Serve a static file.
    Serve(PathBuf),
    /// File not found.
    NotFound,
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
}

/// Trait implemented by each INDEX_FILE mode. The common layer classifies
/// the URI and delegates to one of the three methods based on `UriKind`.
pub(crate) trait ModeRouter: Send + Sync {
    /// Called for `UriKind::NoExtension` URIs.
    fn resolve_no_extension<'a>(
        &'a self,
        sanitized: &'a str,
        ctx: &'a ResolveCtx<'a>,
    ) -> BoxFuture<'a, RouteResult>;

    /// Called for `UriKind::Php` URIs. The common layer does NOT check disk
    /// for these — each mode decides its own rules.
    fn resolve_php<'a>(
        &'a self,
        sanitized: &'a str,
        ctx: &'a ResolveCtx<'a>,
    ) -> BoxFuture<'a, RouteResult>;

    /// Called for `UriKind::OtherExtension` URIs when the common layer's
    /// disk check did NOT find a file. Modes decide the fallback policy.
    fn resolve_static_miss<'a>(
        &'a self,
        sanitized: &'a str,
        ctx: &'a ResolveCtx<'a>,
    ) -> BoxFuture<'a, RouteResult>;
}

/// Routing configuration with mode dispatch and caching layers.
pub struct RouteConfig {
    document_root: Arc<PathBuf>,
    canonical_root: PathBuf,
    mode: Box<dyn ModeRouter>,
    worker_route: Option<RouteResult>,
    /// Cache of resolved routes keyed by URI path. Stored under `RwLock`
    /// so concurrent cache hits take a shared read lock and never contend;
    /// reads use `peek()` to skip LRU promotion (matches `FileCache`). LRU
    /// ordering is only updated on insert, which is rare once the cache
    /// warms — popular entries stay resident because they are re-inserted
    /// after any eviction. Values are `Arc<RouteResult>` so hits are a
    /// single atomic increment rather than a `PathBuf::clone()`.
    route_cache: RwLock<LruCache<String, Arc<RouteResult>>>,
}

impl RouteConfig {
    /// Create route config from server config.
    ///
    /// Panics if the document root cannot be canonicalized, since symlink
    /// escape protection requires a valid, resolvable document root path.
    pub fn new(config: &ServerConfig) -> Self {
        let canonical_root = std::fs::canonicalize(&config.document_root).unwrap_or_else(|e| {
            panic!(
                "Fatal: cannot canonicalize document_root '{}': {}. \
                 Symlink escape protection requires a valid document root path.",
                config.document_root.display(),
                e
            );
        });

        let document_root = Arc::new(config.document_root.clone());

        let mode: Box<dyn ModeRouter> = match config.index_file.as_deref() {
            None | Some("") => Box::new(TraditionalRouter::new(&document_root)),
            Some(name) if name.ends_with(".php") => {
                Box::new(FrameworkRouter::new(&document_root, name))
            }
            Some(name) => Box::new(SpaRouter::new(&document_root, name)),
        };

        Self {
            document_root,
            canonical_root,
            mode,
            worker_route: None,
            route_cache: RwLock::new(LruCache::new(
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

    /// Set the worker file for worker mode routing. When set, all unmatched
    /// requests fall back to this PHP file before returning NotFound.
    pub fn set_worker_file(&mut self, path: PathBuf) {
        self.worker_route = Some(RouteResult::Execute(path, None));
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

        // Fast path: route cache. Read lock + `peek()` — concurrent cache
        // hits don't contend, LRU ordering isn't touched on read.
        {
            let cache = self.route_cache.read().unwrap_or_else(|e| e.into_inner());
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

        // Sanitize path (strip `..`, `.`, empty segments)
        let sanitized = sanitize_path(&decoded);

        // Compute has_php_component() once and share it across the
        // .well-known defence-in-depth check and URI classification.
        let has_php = has_php_component(&sanitized);

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
        };

        let result = match classify_uri_with_php(&sanitized, has_php) {
            UriKind::NoExtension => self.mode.resolve_no_extension(&sanitized, &ctx).await,
            UriKind::Php => self.mode.resolve_php(&sanitized, &ctx).await,
            UriKind::OtherExtension => {
                // Common disk check for non-.php extensions — done once here,
                // shared across all three modes via file_cache.
                let candidate = self.document_root.join(&sanitized);
                if file_cache.is_file(&candidate.to_string_lossy()).await {
                    RouteResult::Serve(candidate)
                } else {
                    self.mode.resolve_static_miss(&sanitized, &ctx).await
                }
            }
        };

        // Symlink-escape protection: the resolved path must live inside
        // the canonical document root. The worker file is an admin-configured
        // trusted path that may sit outside DOCUMENT_ROOT — skip it.
        let result = match &result {
            RouteResult::Serve(path) | RouteResult::Execute(path, _) => {
                let is_worker = self
                    .worker_route
                    .as_ref()
                    .is_some_and(|wr| matches!(wr, RouteResult::Execute(wf, _) if wf == path));
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
            RouteResult::NotFound => result,
        };

        let arc = Arc::new(result);
        self.cache_put(uri_path, &arc);
        arc
    }

    fn cache_put(&self, uri_path: &str, result: &Arc<RouteResult>) {
        let mut cache = self.route_cache.write().unwrap_or_else(|e| e.into_inner());
        cache.put(uri_path.to_string(), Arc::clone(result));
    }

    /// Check that a resolved path stays within the canonical document root.
    /// Results are cached in the file cache to avoid repeated `realpath(3)`.
    async fn validate_path(&self, path: &Path, file_cache: &Arc<FileCache>) -> bool {
        let cache_key = path.to_string_lossy().to_string();

        if let Some(cached) = file_cache.get_canonical(&cache_key) {
            return match cached {
                Some(canonical_path) => canonical_path.starts_with(&self.canonical_root),
                None => true, // file didn't exist at cache time; serve() will 404
            };
        }

        let result = tokio::fs::canonicalize(path).await.ok();
        let valid = match &result {
            Some(canonical_path) => canonical_path.starts_with(&self.canonical_root),
            None => true,
        };

        file_cache.insert_canonical(cache_key, result);
        valid
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
pub(crate) fn has_php_component(sanitized: &str) -> bool {
    let bytes = sanitized.as_bytes();
    if bytes.len() < 4 {
        return false;
    }
    let mut i = 0;
    while i + 4 <= bytes.len() {
        if bytes[i] == b'.'
            && (bytes[i + 1] == b'p' || bytes[i + 1] == b'P')
            && (bytes[i + 2] == b'h' || bytes[i + 2] == b'H')
            && (bytes[i + 3] == b'p' || bytes[i + 3] == b'P')
        {
            let end = i + 4;
            if end == bytes.len() || bytes[end] == b'/' {
                return true;
            }
            i = end;
        } else {
            i += 1;
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
fn sanitize_path(path: &str) -> String {
    let mut result = String::with_capacity(path.len());
    for segment in path.split('/') {
        if !segment.is_empty() && segment != ".." && segment != "." {
            if !result.is_empty() {
                result.push('/');
            }
            result.push_str(segment);
        }
    }
    result
}
