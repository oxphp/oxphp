use std::path::{Path, PathBuf};
use std::sync::Arc;

use percent_encoding::percent_decode_str;

use crate::config::ServerConfig;
use crate::server::response::static_file::FileCache;

/// Result of route resolution.
#[derive(Debug, Clone)]
pub enum RouteResult {
    /// Execute a PHP script (Phase 2+).
    Execute(PathBuf),
    /// Serve a static file.
    Serve(PathBuf),
    /// File not found.
    NotFound,
}

/// Routing configuration derived from server config.
#[derive(Debug, Clone)]
pub struct RouteConfig {
    document_root: PathBuf,
    index_file: Option<String>,
    index_file_path: Option<PathBuf>,
    index_file_is_php: bool,
}

impl RouteConfig {
    /// Create route config from server config.
    pub fn new(config: &ServerConfig) -> Self {
        let index_file_path = config
            .index_file
            .as_ref()
            .map(|f| config.document_root.join(f));

        let index_file_is_php = config
            .index_file
            .as_ref()
            .map(|f| f.ends_with(".php"))
            .unwrap_or(false);

        Self {
            document_root: config.document_root.clone(),
            index_file: config.index_file.clone(),
            index_file_path,
            index_file_is_php,
        }
    }

    /// Resolve a URI path to a route result using the file cache.
    pub async fn resolve_request(
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

        // 6. Root path "/" → resolve with index file
        if uri_path == "/" {
            return self.resolve_index(&self.document_root, file_cache).await;
        }

        // 7. Trailing slash → directory mode
        if uri_path.ends_with('/') {
            return self.resolve_index(&file_path, file_cache).await;
        }

        // 8. File exists → serve/execute
        let path_str = file_path.to_string_lossy();
        if file_cache.is_file(&path_str).await {
            return if file_path.extension().and_then(|s| s.to_str()) == Some("php") {
                RouteResult::Execute(file_path)
            } else {
                RouteResult::Serve(file_path)
            };
        }

        // 9. File not found + INDEX_FILE set → fallback
        if let Some(ref index_path) = self.index_file_path {
            if self.index_file_is_php {
                return RouteResult::Execute(index_path.clone());
            } else {
                return RouteResult::Serve(index_path.clone());
            }
        }

        // 10. Not found
        RouteResult::NotFound
    }

    /// Resolve index file for a directory (tries index.php, then index.html).
    async fn resolve_index(&self, dir: &Path, file_cache: &Arc<FileCache>) -> RouteResult {
        let php_index = dir.join("index.php");
        if file_cache.is_file(&php_index.to_string_lossy()).await {
            return RouteResult::Execute(php_index);
        }

        let html_index = dir.join("index.html");
        if file_cache.is_file(&html_index.to_string_lossy()).await {
            return RouteResult::Serve(html_index);
        }

        RouteResult::NotFound
    }
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
        let config = ServerConfig {
            listen_addr: "0.0.0.0:8080".to_string(),
            document_root: dir.to_path_buf(),
            index_file: index_file.map(|s| s.to_string()),
        };
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
        assert!(matches!(result, RouteResult::Serve(_)));
    }

    #[tokio::test]
    async fn test_traditional_mode_php_file() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/index.php", &cache).await;
        assert!(matches!(result, RouteResult::Execute(_)));
    }

    #[tokio::test]
    async fn test_traditional_mode_root_resolves_index_html() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("index.html"), "hello").unwrap();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/", &cache).await;
        assert!(matches!(result, RouteResult::Serve(_)));
    }

    #[tokio::test]
    async fn test_traditional_mode_root_resolves_index_php_first() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/", &cache).await;
        // index.php exists and is checked first
        assert!(matches!(result, RouteResult::Execute(_)));
    }

    #[tokio::test]
    async fn test_traditional_mode_not_found() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/nonexistent.txt", &cache).await;
        assert!(matches!(result, RouteResult::NotFound));
    }

    // --- Framework mode tests ---

    #[tokio::test]
    async fn test_framework_mode_fallback_to_index_php() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.php"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/unknown/path", &cache).await;
        assert!(matches!(result, RouteResult::Execute(_)));
    }

    #[tokio::test]
    async fn test_framework_mode_blocks_direct_php_access() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.php"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/about.php", &cache).await;
        assert!(matches!(result, RouteResult::NotFound));
    }

    #[tokio::test]
    async fn test_framework_mode_blocks_direct_index_access() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.php"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/index.php", &cache).await;
        assert!(matches!(result, RouteResult::NotFound));
    }

    #[tokio::test]
    async fn test_framework_mode_serves_static_files() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.php"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/style.css", &cache).await;
        assert!(matches!(result, RouteResult::Serve(_)));
    }

    // --- SPA mode tests ---

    #[tokio::test]
    async fn test_spa_mode_fallback_to_index_html() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.html"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/unknown/path", &cache).await;
        assert!(matches!(result, RouteResult::Serve(_)));
    }

    #[tokio::test]
    async fn test_spa_mode_serves_existing_file() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), Some("index.html"));
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/style.css", &cache).await;
        assert!(matches!(result, RouteResult::Serve(_)));
    }

    // --- Security tests ---

    #[tokio::test]
    async fn test_path_traversal_blocked() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/../etc/passwd", &cache).await;
        assert!(matches!(result, RouteResult::NotFound));
    }

    #[tokio::test]
    async fn test_percent_encoded_path() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        // %2e%2e = ".."
        let result = rc.resolve_request("/%2e%2e/etc/passwd", &cache).await;
        assert!(matches!(result, RouteResult::NotFound));
    }

    // --- Subdirectory tests ---

    #[tokio::test]
    async fn test_subdirectory_file() {
        let dir = setup_test_dir();
        let rc = make_config(dir.path(), None);
        let cache = Arc::new(FileCache::new(200));
        let result = rc.resolve_request("/sub/page.html", &cache).await;
        assert!(matches!(result, RouteResult::Serve(_)));
    }
}
