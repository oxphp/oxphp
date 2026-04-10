use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use lru::LruCache;
use percent_encoding::percent_decode_str;

use crate::config::ServerConfig;
use crate::server::response::static_file::FileCache;

/// Maximum number of entries in the route cache.
const ROUTE_CACHE_CAPACITY: usize = 10_000;

/// Result of route resolution.
#[derive(Debug, Clone)]
pub enum RouteResult {
    /// Execute a PHP script. Optional `path_info` carries the extra path
    /// after the `.php` segment (e.g. `/user/42` for `/app.php/user/42`).
    Execute(PathBuf, Option<String>),
    /// Serve a static file.
    Serve(PathBuf),
    /// File not found.
    NotFound,
}

/// Routing configuration derived from server config.
pub struct RouteConfig {
    document_root: Arc<PathBuf>,
    canonical_root: PathBuf,
    index_file: Option<String>,
    index_file_path: Option<PathBuf>,
    index_file_is_php: bool,
    /// Worker mode: pre-computed RouteResult for the worker file.
    /// Avoids PathBuf::clone() heap allocation on every cache miss.
    worker_route: Option<RouteResult>,
    /// Pre-computed root index paths to avoid `join()` on every `/` request.
    root_index_php: PathBuf,
    root_index_html: PathBuf,
    /// Pre-computed string keys for cache lookups (avoids `to_string_lossy()` per request).
    root_index_php_key: String,
    root_index_html_key: String,
    /// When true, URIs like `/script.php/extra/path` are split into
    /// script_path + PATH_INFO instead of being treated as a single path.
    split_path_info: bool,
    /// Cache of resolved routes keyed by URI path.
    /// Mutex is fine here: hold time is O(1) for both get and put.
    /// Stores `Arc<RouteResult>` so cache hits are a single atomic increment
    /// instead of a `PathBuf::clone()` heap allocation.
    route_cache: Mutex<LruCache<String, Arc<RouteResult>>>,
}

impl RouteConfig {
    /// Create route config from server config.
    ///
    /// Panics if the document root cannot be canonicalized, since symlink escape
    /// protection requires a valid, resolvable document root path.
    pub fn new(config: &ServerConfig) -> Self {
        let canonical_root = std::fs::canonicalize(&config.document_root).unwrap_or_else(|e| {
            panic!(
                "Fatal: cannot canonicalize document_root '{}': {}. \
                     Symlink escape protection requires a valid document root path.",
                config.document_root.display(),
                e
            );
        });

        let index_file_path = config
            .index_file
            .as_ref()
            .map(|f| config.document_root.join(f));

        let index_file_is_php = config
            .index_file
            .as_ref()
            .map(|f| f.ends_with(".php"))
            .unwrap_or(false);

        let root_index_php = config.document_root.join("index.php");
        let root_index_html = config.document_root.join("index.html");
        let root_index_php_key = root_index_php.to_string_lossy().into_owned();
        let root_index_html_key = root_index_html.to_string_lossy().into_owned();

        Self {
            document_root: Arc::new(config.document_root.clone()),
            canonical_root,
            index_file: config.index_file.clone(),
            index_file_path,
            index_file_is_php,
            worker_route: None,
            root_index_php,
            root_index_html,
            root_index_php_key,
            root_index_html_key,
            split_path_info: config.split_path_info,
            route_cache: Mutex::new(LruCache::new(
                NonZeroUsize::new(ROUTE_CACHE_CAPACITY).unwrap(),
            )),
        }
    }

    /// Returns the canonical document root, if canonicalization succeeded at startup.
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

    /// Set the worker file for worker mode routing.
    /// When set, all unmatched requests route to this PHP file instead of 404.
    /// Pre-computes the RouteResult to avoid PathBuf::clone() on every cache miss.
    pub fn set_worker_file(&mut self, path: PathBuf) {
        self.worker_route = Some(RouteResult::Execute(path, None));
    }

    /// Resolve a URI path to a route result using the file cache.
    ///
    /// Checks the route cache first. On miss, resolves via the inner logic,
    /// validates that the resolved path does not escape the document root
    /// via symlinks, then caches the result.
    pub async fn resolve_request(
        &self,
        uri_path: &str,
        file_cache: &Arc<FileCache>,
    ) -> Arc<RouteResult> {
        // Fast path: check route cache (single lock, O(1) get + LRU promotion).
        // Returns Arc clone (atomic increment) instead of PathBuf heap allocation.
        {
            let mut cache = self.route_cache.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(result) = cache.get(uri_path) {
                return Arc::clone(result);
            }
        }

        let result = self.resolve_request_inner(uri_path, file_cache).await;

        // Validate resolved path stays within document root.
        // In worker mode, the worker file is an admin-configured trusted path
        // that may live outside the document root — skip validation for it.
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

        let arc_result = Arc::new(result);

        // Cache the result (LruCache handles eviction automatically)
        {
            let mut cache = self.route_cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.put(uri_path.to_string(), Arc::clone(&arc_result));
        }

        arc_result
    }

    /// Check that a resolved path is within the canonical document root.
    /// Results are cached in the file cache to avoid repeated `realpath(3)` syscalls.
    async fn validate_path(&self, path: &Path, file_cache: &Arc<FileCache>) -> bool {
        let canonical_root = &self.canonical_root;

        let cache_key = path.to_string_lossy().to_string();

        // Check cache first
        if let Some(cached) = file_cache.get_canonical(&cache_key) {
            return match cached {
                Some(canonical_path) => canonical_path.starts_with(canonical_root),
                None => true, // file didn't exist at cache time; serve() will 404
            };
        }

        // Cache miss — perform canonicalization
        let result = tokio::fs::canonicalize(path).await.ok();
        let valid = match &result {
            Some(canonical_path) => canonical_path.starts_with(canonical_root),
            None => true, // file doesn't exist; serve() will return 404
        };

        file_cache.insert_canonical(cache_key, result);
        valid
    }

    /// Inner route resolution logic (no path validation).
    async fn resolve_request_inner(
        &self,
        uri_path: &str,
        file_cache: &Arc<FileCache>,
    ) -> RouteResult {
        // 1. Decode URI
        let decoded = match percent_decode_str(uri_path).decode_utf8() {
            Ok(s) => s.to_string(),
            Err(_) => return RouteResult::NotFound,
        };

        // 2. Sanitize path (remove "..")
        let sanitized = sanitize_path(&decoded);

        // 3. Direct access to INDEX_FILE → 404
        if let Some(ref index_file) = self.index_file {
            if sanitized.trim_start_matches('/') == index_file {
                return RouteResult::NotFound;
            }
        }

        // 4. Block direct .php access in framework mode
        if self.index_file_is_php && sanitized.ends_with(".php") {
            return RouteResult::NotFound;
        }

        // 5. Resolve file path
        let file_path = self.document_root.join(sanitized.trim_start_matches('/'));

        // 6. Root path "/" → use pre-computed index paths (no alloc)
        if uri_path == "/" {
            return self.resolve_root_index(file_cache).await;
        }

        // 7. Trailing slash → directory mode
        if uri_path.ends_with('/') {
            return self.resolve_index(&file_path, file_cache).await;
        }

        // 8. File exists → serve/execute
        let path_str = file_path.to_string_lossy();
        if file_cache.is_file(&path_str).await {
            return if file_path.extension().and_then(|s| s.to_str()) == Some("php") {
                RouteResult::Execute(file_path, None)
            } else {
                RouteResult::Serve(file_path)
            };
        }

        // 8b. SPLIT_PATH_INFO: walk path segments to find a .php file prefix
        if self.split_path_info {
            if let Some(result) = self.try_split_path_info(&sanitized, file_cache).await {
                return result;
            }
        }

        // 9. Worker mode → all unmatched requests go to the worker
        if let Some(ref wr) = self.worker_route {
            return wr.clone();
        }

        // 10. File not found + INDEX_FILE set → fallback
        if let Some(ref index_path) = self.index_file_path {
            if self.index_file_is_php {
                return RouteResult::Execute(index_path.clone(), None);
            } else {
                return RouteResult::Serve(index_path.clone());
            }
        }

        // 11. Not found
        RouteResult::NotFound
    }

    /// Resolve root `/` using pre-computed index paths and string keys (zero allocation).
    async fn resolve_root_index(&self, file_cache: &Arc<FileCache>) -> RouteResult {
        // Framework/SPA mode: root goes to INDEX_FILE
        if let Some(ref index_path) = self.index_file_path {
            if self.index_file_is_php {
                return RouteResult::Execute(index_path.clone(), None);
            } else {
                return RouteResult::Serve(index_path.clone());
            }
        }

        // Traditional mode: check index.php, then index.html
        if file_cache.is_file(&self.root_index_php_key).await {
            return RouteResult::Execute(self.root_index_php.clone(), None);
        }

        if file_cache.is_file(&self.root_index_html_key).await {
            return RouteResult::Serve(self.root_index_html.clone());
        }

        // Worker mode: root goes to worker file
        if let Some(ref wr) = self.worker_route {
            return wr.clone();
        }

        RouteResult::NotFound
    }

    /// Resolve index file for a subdirectory (tries index.php, then index.html).
    async fn resolve_index(&self, dir: &Path, file_cache: &Arc<FileCache>) -> RouteResult {
        let php_index = dir.join("index.php");
        if file_cache.is_file(&php_index.to_string_lossy()).await {
            return RouteResult::Execute(php_index, None);
        }

        let html_index = dir.join("index.html");
        if file_cache.is_file(&html_index.to_string_lossy()).await {
            return RouteResult::Serve(html_index);
        }

        // Worker mode: subdirectory without index goes to worker
        if let Some(ref wr) = self.worker_route {
            return wr.clone();
        }

        // Framework/SPA mode: fallback to INDEX_FILE
        if let Some(ref index_path) = self.index_file_path {
            if self.index_file_is_php {
                return RouteResult::Execute(index_path.clone(), None);
            } else {
                return RouteResult::Serve(index_path.clone());
            }
        }

        RouteResult::NotFound
    }

    /// Try to split a URI path into a PHP script and PATH_INFO.
    /// Iterates segments left-to-right looking for a `.php` file on disk.
    /// Returns `Some(Execute(script, path_info))` on match, `None` otherwise.
    async fn try_split_path_info(
        &self,
        sanitized: &str,
        file_cache: &Arc<FileCache>,
    ) -> Option<RouteResult> {
        // Look for `.php` within the path — quick bail if none
        if !sanitized.contains(".php") {
            return None;
        }

        // Find each `.php` boundary and check if it's a real file
        let mut search_from = 0;
        while let Some(pos) = sanitized[search_from..].find(".php") {
            let end = search_from + pos + 4; // past ".php"

            // The `.php` must be at end-of-string or followed by `/`
            if end < sanitized.len() && sanitized.as_bytes()[end] != b'/' {
                search_from = end;
                continue;
            }

            let script_part = &sanitized[..end];
            let candidate = self.document_root.join(script_part);
            let candidate_str = candidate.to_string_lossy();

            if file_cache.is_file(&candidate_str).await {
                let path_info = if end < sanitized.len() {
                    // Everything after the .php segment (starts with `/`)
                    Some(sanitized[end..].to_string())
                } else {
                    None
                };
                return Some(RouteResult::Execute(candidate, path_info));
            }

            search_from = end;
        }

        None
    }
}

/// Returns true if the URI path contains a blocked dot-segment.
///
/// A dot-segment is any path component starting with `.` (e.g. `.git`, `.env`).
/// Exception: `.well-known` as the **first** segment with a non-empty sub-path
/// (`/.well-known/foo`) is allowed per RFC 8615. Bare `/.well-known` and
/// `/.well-known/` are blocked.
///
/// The check runs on percent-decoded input to catch encoded bypasses like `/%2egit/`.
#[allow(dead_code)] // wired into resolve_request in a subsequent commit
fn is_blocked_dot_path(uri_path: &str) -> bool {
    let decoded = match percent_decode_str(uri_path).decode_utf8() {
        Ok(s) => s,
        Err(_) => return true, // invalid UTF-8 → block
    };

    let mut segments = decoded.split('/').filter(|s| !s.is_empty());
    let mut is_first = true;

    while let Some(seg) = segments.next() {
        if !seg.starts_with('.') {
            is_first = false;
            continue;
        }

        // .well-known exception: must be first segment AND have a non-empty sub-path
        if seg == ".well-known" && is_first {
            match segments.next() {
                Some(next) if !next.is_empty() && !next.starts_with('.') => {
                    // .well-known sub-path allowed, but check remaining segments
                    is_first = false;
                    continue;
                }
                _ => return true, // bare .well-known, .well-known/, or dot-segment after → blocked
            }
        }

        return true; // any other dot-segment → blocked
    }

    false
}

/// Remove `..` and empty segments from a path to prevent directory traversal.
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("index.html"), "<html>Hello</html>").unwrap();
        fs::write(dir.path().join("style.css"), "body {}").unwrap();
        fs::write(dir.path().join("index.php"), "<?php echo 'hi';").unwrap();
        fs::write(dir.path().join("about.php"), "<?php echo 'about';").unwrap();
        fs::create_dir_all(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/page.html"), "<html>Sub</html>").unwrap();
        dir
    }

    fn make_config(dir: &Path, index_file: Option<&str>) -> RouteConfig {
        let config = ServerConfig::new(
            "0.0.0.0:8080".to_string(),
            dir.to_path_buf(),
            index_file.map(|s| s.to_string()),
        );
        RouteConfig::new(&config)
    }

    // --- sanitize_path tests ---

    #[test]
    fn test_sanitize_path_removes_dotdot() {
        assert_eq!(sanitize_path("/foo/../bar"), "foo/bar");
    }

    #[test]
    fn test_sanitize_path_removes_empty_segments() {
        assert_eq!(sanitize_path("/foo//bar"), "foo/bar");
    }

    #[test]
    fn test_sanitize_path_removes_dot() {
        assert_eq!(sanitize_path("/foo/./bar"), "foo/bar");
    }

    // --- Traditional mode tests ---

    #[tokio::test]
    async fn test_traditional_mode_static_file() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/style.css", &cache).await;
        assert!(matches!(*result, RouteResult::Serve(_)));
    }

    #[tokio::test]
    async fn test_traditional_mode_php_file() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/index.php", &cache).await;
        assert!(matches!(*result, RouteResult::Execute(..)));
    }

    #[tokio::test]
    async fn test_traditional_mode_root_resolves_index_html() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("index.html"), "hello").unwrap();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/", &cache).await;
        assert!(matches!(*result, RouteResult::Serve(_)));
    }

    #[tokio::test]
    async fn test_traditional_mode_root_resolves_index_php_first() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/", &cache).await;
        // index.php exists and is checked first
        assert!(matches!(*result, RouteResult::Execute(..)));
    }

    #[tokio::test]
    async fn test_traditional_mode_not_found() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/nonexistent.txt", &cache).await;
        assert!(matches!(*result, RouteResult::NotFound));
    }

    // --- Framework mode tests ---

    #[tokio::test]
    async fn test_framework_mode_fallback_to_index_php() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.php"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/unknown/path", &cache).await;
        assert!(matches!(*result, RouteResult::Execute(..)));
    }

    #[tokio::test]
    async fn test_framework_mode_blocks_direct_php_access() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.php"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/about.php", &cache).await;
        assert!(matches!(*result, RouteResult::NotFound));
    }

    #[tokio::test]
    async fn test_framework_mode_blocks_direct_index_access() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.php"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/index.php", &cache).await;
        assert!(matches!(*result, RouteResult::NotFound));
    }

    #[tokio::test]
    async fn test_framework_mode_serves_static_files() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.php"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/style.css", &cache).await;
        assert!(matches!(*result, RouteResult::Serve(_)));
    }

    // --- SPA mode tests ---

    #[tokio::test]
    async fn test_spa_mode_fallback_to_index_html() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.html"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/unknown/path", &cache).await;
        assert!(matches!(*result, RouteResult::Serve(_)));
    }

    #[tokio::test]
    async fn test_spa_mode_serves_existing_file() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.html"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/style.css", &cache).await;
        assert!(matches!(*result, RouteResult::Serve(_)));
    }

    // --- Security tests ---

    #[tokio::test]
    async fn test_path_traversal_blocked() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/../etc/passwd", &cache).await;
        assert!(matches!(*result, RouteResult::NotFound));
    }

    #[tokio::test]
    async fn test_percent_encoded_path() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        // %2e%2e = ".."
        let result = rc.resolve_request("/%2e%2e/etc/passwd", &cache).await;
        assert!(matches!(*result, RouteResult::NotFound));
    }

    // --- Symlink escape test ---

    #[cfg(unix)]
    #[tokio::test]
    async fn test_symlink_escape_blocked() {
        use std::os::unix::fs::symlink;

        let dir = setup_test_dir();
        // Create a symlink inside document_root pointing outside it
        let target = TempDir::new().unwrap();
        fs::write(target.path().join("secret.txt"), "secret data").unwrap();
        symlink(target.path(), dir.path().join("escape")).unwrap();

        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/escape/secret.txt", &cache).await;
        assert!(
            matches!(*result, RouteResult::NotFound),
            "Symlink escape should be blocked"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn test_symlink_escape_cached_on_second_request() {
        use std::os::unix::fs::symlink;

        let dir = setup_test_dir();
        let target = TempDir::new().unwrap();
        fs::write(target.path().join("secret.txt"), "secret data").unwrap();
        symlink(target.path(), dir.path().join("escape")).unwrap();

        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));

        // First request: cache miss, canonicalize, block
        let result1 = rc.resolve_request("/escape/secret.txt", &cache).await;
        assert!(matches!(*result1, RouteResult::NotFound));

        // Second request: should hit the canonical cache, still blocked
        let result2 = rc.resolve_request("/escape/secret.txt", &cache).await;
        assert!(matches!(*result2, RouteResult::NotFound));

        // Verify the canonical path was cached
        let escaped_path = dir.path().join("escape/secret.txt");
        let cached = cache.get_canonical(&escaped_path.to_string_lossy());
        assert!(cached.is_some(), "Canonical path should be cached");
    }

    // --- Framework mode root with custom INDEX_FILE ---

    #[tokio::test]
    async fn test_framework_mode_root_uses_custom_index_file() {
        let dir = setup_test_dir();
        fs::write(dir.path().join("index.php"), "<?php echo 'index';").unwrap();
        let rc = make_config(dir.path(), Some("index.php"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/", &cache).await;
        match &*result {
            RouteResult::Execute(path, _) => {
                assert!(
                    path.ends_with("index.php"),
                    "Root should route to index.php, got {:?}",
                    path
                );
            }
            other => panic!("Expected Execute(index.php), got {:?}", other),
        }
    }

    // --- Framework mode trailing slash fallback ---

    #[tokio::test]
    async fn test_framework_mode_trailing_slash_falls_back_to_index_php() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.php"));
        let cache = Arc::new(FileCache::new(200));
        // /_profiler/ has trailing slash, no _profiler/index.php exists
        let result = rc.resolve_request("/_profiler/", &cache).await;
        match &*result {
            RouteResult::Execute(path, _) => {
                assert!(
                    path.ends_with("index.php"),
                    "Should fallback to index.php, got {:?}",
                    path
                );
            }
            other => panic!("Expected Execute(index.php), got {:?}", other),
        }
    }

    // --- Subdirectory tests ---

    #[tokio::test]
    async fn test_subdirectory_file() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/sub/page.html", &cache).await;
        assert!(matches!(*result, RouteResult::Serve(_)));
    }

    #[tokio::test]
    async fn test_route_cache_capacity_cap() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));

        // Fill route cache beyond capacity with unique paths
        for i in 0..ROUTE_CACHE_CAPACITY + 100 {
            rc.resolve_request(&format!("/nonexistent_{i}.txt"), &cache)
                .await;
        }

        // Route cache should not exceed capacity (LRU eviction keeps it bounded)
        let cache_len = rc.route_cache.lock().unwrap().len();
        assert!(
            cache_len <= ROUTE_CACHE_CAPACITY,
            "Route cache size {} exceeds capacity {}",
            cache_len,
            ROUTE_CACHE_CAPACITY,
        );
    }

    #[tokio::test]
    async fn test_route_cache_lru_eviction() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));

        // Access /style.css to warm it into cache
        rc.resolve_request("/style.css", &cache).await;

        // Fill cache to capacity with other paths
        for i in 0..ROUTE_CACHE_CAPACITY {
            rc.resolve_request(&format!("/fill_{i}.txt"), &cache).await;
        }

        // /style.css was least recently used — should be evicted
        let lru_cache = rc.route_cache.lock().unwrap();
        assert!(
            !lru_cache.contains("/style.css"),
            "LRU entry should have been evicted"
        );
        assert!(
            lru_cache.len() <= ROUTE_CACHE_CAPACITY,
            "Cache should not exceed capacity"
        );
    }

    // --- SPLIT_PATH_INFO tests ---

    fn make_config_with_split(dir: &Path, index_file: Option<&str>) -> RouteConfig {
        let mut config = ServerConfig::new(
            "0.0.0.0:8080".to_string(),
            dir.to_path_buf(),
            index_file.map(|s| s.to_string()),
        );
        config.split_path_info = true;
        RouteConfig::new(&config)
    }

    #[tokio::test]
    async fn test_split_path_info_basic() {
        let dir = setup_test_dir();
        let rc = make_config_with_split(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        // /about.php/user/42 → script=about.php, path_info=/user/42
        let result = rc.resolve_request("/about.php/user/42", &cache).await;
        match &*result {
            RouteResult::Execute(path, Some(pi)) => {
                assert!(path.ends_with("about.php"), "got {:?}", path);
                assert_eq!(pi, "/user/42");
            }
            other => panic!("Expected Execute with path_info, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_split_path_info_no_extra_path() {
        let dir = setup_test_dir();
        let rc = make_config_with_split(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        // /about.php → normal execute, no path_info
        let result = rc.resolve_request("/about.php", &cache).await;
        match &*result {
            RouteResult::Execute(path, None) => {
                assert!(path.ends_with("about.php"), "got {:?}", path);
            }
            other => panic!("Expected Execute without path_info, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_split_path_info_disabled_returns_not_found() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None); // split_path_info = false
        let cache = Arc::new(FileCache::new(200));
        // Without splitting, /about.php/user/42 is a single path → not found
        let result = rc.resolve_request("/about.php/user/42", &cache).await;
        assert!(
            matches!(*result, RouteResult::NotFound),
            "Without split, path with extra segments should 404"
        );
    }

    #[tokio::test]
    async fn test_split_path_info_nonexistent_script() {
        let dir = setup_test_dir();
        let rc = make_config_with_split(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        // /missing.php/foo → script doesn't exist → not found
        let result = rc.resolve_request("/missing.php/foo", &cache).await;
        assert!(matches!(*result, RouteResult::NotFound));
    }

    #[tokio::test]
    async fn test_split_path_info_deep_path() {
        let dir = setup_test_dir();
        let rc = make_config_with_split(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc
            .resolve_request("/index.php/api/v2/users/42/profile", &cache)
            .await;
        match &*result {
            RouteResult::Execute(path, Some(pi)) => {
                assert!(path.ends_with("index.php"), "got {:?}", path);
                assert_eq!(pi, "/api/v2/users/42/profile");
            }
            other => panic!("Expected Execute with deep path_info, got {:?}", other),
        }
    }

    // --- Dot-path blocking tests ---

    #[test]
    fn test_dot_path_blocks_dot_env() {
        assert!(is_blocked_dot_path("/.env"));
    }

    #[test]
    fn test_dot_path_blocks_dot_git_subpath() {
        assert!(is_blocked_dot_path("/.git/config"));
    }

    #[test]
    fn test_dot_path_blocks_htaccess() {
        assert!(is_blocked_dot_path("/.htaccess"));
    }

    #[test]
    fn test_dot_path_blocks_ds_store() {
        assert!(is_blocked_dot_path("/.DS_Store"));
    }

    #[test]
    fn test_dot_path_blocks_mid_path_dot_segment() {
        assert!(is_blocked_dot_path("/path/.hidden/file.txt"));
    }

    #[test]
    fn test_dot_path_blocks_deep_dot_file() {
        assert!(is_blocked_dot_path("/path/to/.env"));
    }

    #[test]
    fn test_dot_path_blocks_encoded_dot_segment() {
        // %2e = "."
        assert!(is_blocked_dot_path("/%2egit/HEAD"));
    }

    #[test]
    fn test_dot_path_blocks_encoded_dot_env() {
        assert!(is_blocked_dot_path("/%2eenv"));
    }

    #[test]
    fn test_dot_path_allows_well_known_subpath() {
        assert!(!is_blocked_dot_path("/.well-known/security.txt"));
    }

    #[test]
    fn test_dot_path_allows_well_known_deep_subpath() {
        assert!(!is_blocked_dot_path("/.well-known/acme-challenge/token123"));
    }

    #[test]
    fn test_dot_path_blocks_bare_well_known() {
        assert!(is_blocked_dot_path("/.well-known"));
    }

    #[test]
    fn test_dot_path_blocks_well_known_trailing_slash() {
        assert!(is_blocked_dot_path("/.well-known/"));
    }

    #[test]
    fn test_dot_path_blocks_well_known_not_at_root() {
        assert!(is_blocked_dot_path("/subdir/.well-known/foo"));
    }

    #[test]
    fn test_dot_path_allows_normal_paths() {
        assert!(!is_blocked_dot_path("/style.css"));
        assert!(!is_blocked_dot_path("/index.php"));
        assert!(!is_blocked_dot_path("/path/to/file.txt"));
        assert!(!is_blocked_dot_path("/"));
        assert!(!is_blocked_dot_path("/api/v2/users"));
    }

    #[test]
    fn test_dot_path_allows_dots_in_filenames() {
        assert!(!is_blocked_dot_path("/file.name.with.dots.txt"));
        assert!(!is_blocked_dot_path("/jquery.min.js"));
    }

    #[test]
    fn test_dot_path_blocks_well_known_dot_segment_after() {
        assert!(is_blocked_dot_path("/.well-known/.secret/file"));
    }
}
