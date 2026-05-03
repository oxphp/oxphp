use std::fs;
use std::path::Path;
use std::sync::Arc;

use tempfile::TempDir;

use super::*;
use crate::config::ServerConfig;
use crate::server::response::static_file::FileCache;

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

fn make_config(dir: &Path, entry_file: Option<&str>) -> RouteConfig {
    let config = ServerConfig::new("0.0.0.0:8080".to_string(), dir.to_path_buf());
    let entry_path = entry_file.map(|name| dir.join(name));
    RouteConfig::new(&config, entry_path.as_deref(), false)
}

/// Test helper that mirrors the dot-path screen inside `resolve_request`:
/// byte fast-path, percent-decode on demand, then segment check.
fn is_blocked_uri(uri: &str) -> bool {
    if !has_dot_segment_markers(uri) {
        return false;
    }
    match percent_decode_str(uri).decode_utf8() {
        Ok(s) => contains_blocked_dot_segment(&s),
        Err(_) => true,
    }
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

// --- classify_uri / has_php_component tests ---

#[test]
fn test_classify_root_is_no_extension() {
    assert_eq!(classify_uri(""), UriKind::NoExtension);
}

#[test]
fn test_classify_no_extension_path() {
    assert_eq!(classify_uri("api/users"), UriKind::NoExtension);
    assert_eq!(classify_uri("foo"), UriKind::NoExtension);
}

#[test]
fn test_classify_php_extension() {
    assert_eq!(classify_uri("about.php"), UriKind::Php);
    assert_eq!(classify_uri("sub/dir/script.php"), UriKind::Php);
}

#[test]
fn test_classify_php_case_insensitive() {
    assert_eq!(classify_uri("about.PHP"), UriKind::Php);
    assert_eq!(classify_uri("about.Php"), UriKind::Php);
}

#[test]
fn test_classify_php_with_path_info() {
    assert_eq!(classify_uri("about.php/user/42"), UriKind::Php);
    assert_eq!(classify_uri("api.PHP/v1/users"), UriKind::Php);
}

#[test]
fn test_classify_other_extension() {
    assert_eq!(classify_uri("style.css"), UriKind::OtherExtension);
    assert_eq!(classify_uri("logo.png"), UriKind::OtherExtension);
    assert_eq!(classify_uri("archive.tar.gz"), UriKind::OtherExtension);
}

#[test]
fn test_classify_non_alphanumeric_extension_is_no_extension() {
    // Nginx regex [a-zA-Z0-9]+ won't match e.g. "foo.bar-baz"
    assert_eq!(classify_uri("foo.bar-baz"), UriKind::NoExtension);
}

#[test]
fn test_classify_dot_prefix_is_no_extension() {
    // `.env`-style — treated as no extension (dot-paths blocked upstream anyway)
    assert_eq!(classify_uri(".env"), UriKind::NoExtension);
}

#[test]
fn test_classify_phperror_is_not_php() {
    // `.phperror` extension must not match php component
    assert_eq!(classify_uri("foo.phperror"), UriKind::OtherExtension);
}

#[test]
fn test_classify_docs_php_backup_is_not_php() {
    // ".php" inside the middle of the extension, not a real php component
    assert_eq!(classify_uri("docs.php.backup"), UriKind::OtherExtension);
}

#[test]
fn test_has_php_component_true() {
    assert!(has_php_component("foo.php"));
    assert!(has_php_component("foo.php/bar"));
    assert!(has_php_component("deep/path/to/script.PHP"));
}

#[test]
fn test_has_php_component_false() {
    assert!(!has_php_component("foo.phperror"));
    assert!(!has_php_component("docs.php.backup"));
    assert!(!has_php_component(""));
    assert!(!has_php_component("no-dot"));
}

// --- Traditional mode tests ---

#[tokio::test]
async fn test_traditional_static_file() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/style.css", &cache).await;
    assert!(matches!(*result, RouteResult::Serve(_)));
}

#[tokio::test]
async fn test_traditional_direct_php_file() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/about.php", &cache).await;
    match &*result {
        RouteResult::Execute(path, None, None) => {
            assert!(path.ends_with("about.php"), "got {:?}", path);
        }
        other => panic!("Expected Execute(about.php), got {:?}", other),
    }
}

#[tokio::test]
async fn test_traditional_root_prefers_index_php() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/", &cache).await;
    match &*result {
        RouteResult::Execute(path, _, _) => {
            assert!(path.ends_with("index.php"), "got {:?}", path);
        }
        other => panic!("Expected Execute(index.php), got {:?}", other),
    }
}

#[tokio::test]
async fn test_traditional_root_serves_index_html_when_no_php() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("index.html"), "hello").unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/", &cache).await;
    assert!(matches!(*result, RouteResult::Serve(_)));
}

#[tokio::test]
async fn test_traditional_missing_static_falls_back_to_index_php() {
    // try_files $uri $uri/ /index.php /index.html =404 — missing .txt
    // with non-existent file falls through to /index.php.
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/nonexistent.txt", &cache).await;
    match &*result {
        RouteResult::Execute(path, _, _) => {
            assert!(path.ends_with("index.php"), "got {:?}", path);
        }
        other => panic!("Expected fallback to index.php, got {:?}", other),
    }
}

#[tokio::test]
async fn test_traditional_missing_no_extension_falls_back_to_index_php() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/some/unknown/route", &cache).await;
    match &*result {
        RouteResult::Execute(path, _, _) => {
            assert!(path.ends_with("index.php"), "got {:?}", path);
        }
        other => panic!("Expected fallback to index.php, got {:?}", other),
    }
}

#[tokio::test]
async fn test_traditional_not_found_without_index_files() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("other.txt"), "data").unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/nothing", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));
}

#[tokio::test]
async fn test_traditional_split_path_info_basic() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/about.php/user/42", &cache).await;
    match &*result {
        RouteResult::Execute(path, Some(pi), _) => {
            assert!(path.ends_with("about.php"), "got {:?}", path);
            assert_eq!(pi, "/user/42");
        }
        other => panic!("Expected Execute with path_info, got {:?}", other),
    }
}

#[tokio::test]
async fn test_traditional_split_path_info_deep() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc
        .resolve_request("/index.php/api/v2/users/42/profile", &cache)
        .await;
    match &*result {
        RouteResult::Execute(path, Some(pi), _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(pi, "/api/v2/users/42/profile");
        }
        other => panic!("Expected deep path_info, got {:?}", other),
    }
}

#[tokio::test]
async fn test_traditional_split_path_info_missing_script_falls_back() {
    // /missing.php/foo → missing.php doesn't exist → fall through to /index.php
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/missing.php/foo", &cache).await;
    match &*result {
        RouteResult::Execute(path, _, _) => {
            assert!(path.ends_with("index.php"));
        }
        other => panic!("Expected fallback to index.php, got {:?}", other),
    }
}

#[tokio::test]
async fn test_traditional_subdirectory_file() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/sub/page.html", &cache).await;
    assert!(matches!(*result, RouteResult::Serve(_)));
}

#[tokio::test]
async fn test_traditional_directory_with_index_html() {
    let dir = setup_test_dir();
    fs::write(dir.path().join("sub/index.html"), "<html>sub</html>").unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/sub", &cache).await;
    match &*result {
        RouteResult::Serve(path) => {
            assert!(path.ends_with("sub/index.html"), "got {:?}", path);
        }
        other => panic!("Expected Serve(sub/index.html), got {:?}", other),
    }
}

#[tokio::test]
async fn test_traditional_directory_with_index_php() {
    let dir = setup_test_dir();
    fs::write(dir.path().join("sub/index.php"), "<?php echo 'sub';").unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/sub", &cache).await;
    match &*result {
        RouteResult::Execute(path, None, None) => {
            assert!(path.ends_with("sub/index.php"), "got {:?}", path);
        }
        other => panic!("Expected Execute(sub/index.php), got {:?}", other),
    }
}

// --- Framework mode tests ---

#[tokio::test]
async fn test_framework_unknown_route_goes_to_index_php() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/unknown/path", &cache).await;
    match &*result {
        RouteResult::Execute(path, Some(pi), _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(pi, "/unknown/path");
        }
        other => panic!("Expected Execute with PATH_INFO, got {:?}", other),
    }
}

#[tokio::test]
async fn test_framework_root_goes_to_index_php_with_slash_path_info() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/", &cache).await;
    match &*result {
        RouteResult::Execute(path, Some(pi), _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(pi, "/");
        }
        other => panic!("Expected Execute with PATH_INFO=/, got {:?}", other),
    }
}

#[tokio::test]
async fn test_framework_direct_php_rewrites_to_index_php() {
    // NEW behavior: any `.php` request — including files that exist on disk —
    // gets rewritten to the front controller.
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/about.php", &cache).await;
    match &*result {
        RouteResult::Execute(path, Some(pi), _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(pi, "/about.php");
        }
        other => panic!("Expected rewrite to index.php, got {:?}", other),
    }
}

#[tokio::test]
async fn test_framework_direct_index_php_allowed() {
    // NEW behavior: direct access to the front controller no longer 404s.
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/index.php", &cache).await;
    match &*result {
        RouteResult::Execute(path, Some(pi), _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(pi, "/index.php");
        }
        other => panic!("Expected Execute(index.php), got {:?}", other),
    }
}

#[tokio::test]
async fn test_framework_static_file_served() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/style.css", &cache).await;
    assert!(matches!(*result, RouteResult::Serve(_)));
}

#[tokio::test]
async fn test_framework_missing_static_hard_404() {
    // Non-.php extension with no file on disk → hard 404 (no fallback)
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/missing.png", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));
}

#[tokio::test]
async fn test_framework_php_path_info() {
    // /api.php/v1/users → rewrite to index.php with PATH_INFO=/api.php/v1/users
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/api.php/v1/users", &cache).await;
    match &*result {
        RouteResult::Execute(path, Some(pi), _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(pi, "/api.php/v1/users");
        }
        other => panic!("Expected index.php rewrite, got {:?}", other),
    }
}

// --- SPA mode tests ---

#[tokio::test]
async fn test_spa_unknown_route_serves_index_html() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.html"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/unknown/path", &cache).await;
    match &*result {
        RouteResult::Serve(path) => {
            assert!(path.ends_with("index.html"), "got {:?}", path);
        }
        other => panic!("Expected Serve(index.html), got {:?}", other),
    }
}

#[tokio::test]
async fn test_spa_root_serves_index_html() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.html"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/", &cache).await;
    assert!(matches!(*result, RouteResult::Serve(_)));
}

#[tokio::test]
async fn test_spa_static_file_served() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.html"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/style.css", &cache).await;
    assert!(matches!(*result, RouteResult::Serve(_)));
}

#[tokio::test]
async fn test_spa_missing_static_hard_404() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.html"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/missing.png", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));
}

#[tokio::test]
async fn test_spa_direct_index_html_allowed() {
    // NEW behavior: direct access to index.html is no longer blocked
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.html"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/index.html", &cache).await;
    assert!(matches!(*result, RouteResult::Serve(_)));
}

#[tokio::test]
async fn test_spa_existing_php_executes() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.html"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/about.php", &cache).await;
    match &*result {
        RouteResult::Execute(path, None, None) => {
            assert!(path.ends_with("about.php"));
        }
        other => panic!("Expected Execute(about.php), got {:?}", other),
    }
}

#[tokio::test]
async fn test_spa_missing_php_hard_404() {
    // NEW behavior: missing .php no longer falls through to index.html — 404
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.html"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/missing.php", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));
}

#[tokio::test]
async fn test_spa_php_with_path_info_hard_404_when_missing() {
    // /missing.php/foo → .php component → resolve_php → file doesn't exist → 404
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.html"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/missing.php/foo", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));
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
async fn test_percent_encoded_traversal_blocked() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/%2e%2e/etc/passwd", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));
}

#[cfg(unix)]
#[tokio::test]
async fn test_symlink_escape_blocked() {
    use std::os::unix::fs::symlink;

    let dir = setup_test_dir();
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

    let result1 = rc.resolve_request("/escape/secret.txt", &cache).await;
    assert!(matches!(*result1, RouteResult::NotFound));

    let result2 = rc.resolve_request("/escape/secret.txt", &cache).await;
    assert!(matches!(*result2, RouteResult::NotFound));

    let escaped_path = dir.path().join("escape/secret.txt");
    let cached = cache.get_canonical(&escaped_path.to_string_lossy());
    assert!(cached.is_some(), "Canonical path should be cached");
}

// --- Route cache tests ---

#[tokio::test]
async fn test_route_cache_capacity_cap() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));

    for i in 0..ROUTE_CACHE_CAPACITY + 100 {
        rc.resolve_request(&format!("/nonexistent_{i}.txt"), &cache)
            .await;
    }

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

    rc.resolve_request("/style.css", &cache).await;

    for i in 0..ROUTE_CACHE_CAPACITY {
        rc.resolve_request(&format!("/fill_{i}.txt"), &cache).await;
    }

    let lru_cache = rc.route_cache.lock().unwrap();
    assert!(
        !lru_cache.contains("/style.css"),
        "LRU entry should have been evicted"
    );
    assert!(lru_cache.len() <= ROUTE_CACHE_CAPACITY);
}

// --- Dot-path blocking tests ---

#[test]
fn test_dot_path_blocks_dot_env() {
    assert!(is_blocked_uri("/.env"));
}

#[test]
fn test_dot_path_blocks_dot_git_subpath() {
    assert!(is_blocked_uri("/.git/config"));
}

#[test]
fn test_dot_path_blocks_htaccess() {
    assert!(is_blocked_uri("/.htaccess"));
}

#[test]
fn test_dot_path_blocks_ds_store() {
    assert!(is_blocked_uri("/.DS_Store"));
}

#[test]
fn test_dot_path_blocks_mid_path_dot_segment() {
    assert!(is_blocked_uri("/path/.hidden/file.txt"));
}

#[test]
fn test_dot_path_blocks_deep_dot_file() {
    assert!(is_blocked_uri("/path/to/.env"));
}

#[test]
fn test_dot_path_blocks_encoded_dot_segment() {
    assert!(is_blocked_uri("/%2egit/HEAD"));
}

#[test]
fn test_dot_path_blocks_encoded_dot_env() {
    assert!(is_blocked_uri("/%2eenv"));
}

#[test]
fn test_dot_path_allows_well_known_subpath() {
    assert!(!is_blocked_uri("/.well-known/security.txt"));
}

#[test]
fn test_dot_path_allows_well_known_deep_subpath() {
    assert!(!is_blocked_uri("/.well-known/acme-challenge/token123"));
}

#[test]
fn test_dot_path_blocks_bare_well_known() {
    assert!(is_blocked_uri("/.well-known"));
}

#[test]
fn test_dot_path_blocks_well_known_trailing_slash() {
    assert!(is_blocked_uri("/.well-known/"));
}

#[test]
fn test_dot_path_blocks_well_known_not_at_root() {
    assert!(is_blocked_uri("/subdir/.well-known/foo"));
}

#[test]
fn test_dot_path_allows_normal_paths() {
    assert!(!is_blocked_uri("/style.css"));
    assert!(!is_blocked_uri("/index.php"));
    assert!(!is_blocked_uri("/path/to/file.txt"));
    assert!(!is_blocked_uri("/"));
    assert!(!is_blocked_uri("/api/v2/users"));
}

#[test]
fn test_dot_path_allows_dots_in_filenames() {
    assert!(!is_blocked_uri("/file.name.with.dots.txt"));
    assert!(!is_blocked_uri("/jquery.min.js"));
}

#[test]
fn test_dot_path_blocks_well_known_dot_segment_after() {
    assert!(is_blocked_uri("/.well-known/.secret/file"));
}

#[test]
fn test_dot_path_blocks_well_known_deep_dot_segment() {
    assert!(is_blocked_uri("/.well-known/valid/.hidden"));
}

// --- Dot-path routing integration tests ---

#[tokio::test]
async fn test_resolve_blocks_dot_env() {
    let dir = setup_test_dir();
    fs::write(dir.path().join(".env"), "SECRET=value").unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/.env", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));
}

#[tokio::test]
async fn test_resolve_blocks_dot_git_config() {
    let dir = setup_test_dir();
    fs::create_dir_all(dir.path().join(".git")).unwrap();
    fs::write(dir.path().join(".git/config"), "[core]").unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/.git/config", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));
}

#[tokio::test]
async fn test_resolve_blocks_encoded_dot_path() {
    let dir = setup_test_dir();
    fs::write(dir.path().join(".env"), "SECRET=value").unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/%2eenv", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));
}

#[tokio::test]
async fn test_resolve_allows_well_known_static_file() {
    let dir = setup_test_dir();
    fs::create_dir_all(dir.path().join(".well-known")).unwrap();
    fs::write(
        dir.path().join(".well-known/security.txt"),
        "Contact: security@example.com",
    )
    .unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc
        .resolve_request("/.well-known/security.txt", &cache)
        .await;
    assert!(
        matches!(*result, RouteResult::Serve(_)),
        "Expected Serve, got {:?}",
        result
    );
}

#[tokio::test]
async fn test_resolve_blocks_bare_well_known() {
    let dir = setup_test_dir();
    fs::create_dir_all(dir.path().join(".well-known")).unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/.well-known", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));
}

#[tokio::test]
async fn test_resolve_dot_path_not_cached() {
    let dir = setup_test_dir();
    fs::write(dir.path().join(".env"), "SECRET=value").unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));

    let result = rc.resolve_request("/.env", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));

    let route_cache = rc.route_cache.lock().unwrap();
    assert!(
        !route_cache.contains("/.env"),
        "Blocked dot-paths must not pollute the route cache"
    );
}

#[tokio::test]
async fn test_resolve_dot_path_blocked_in_framework_mode() {
    let dir = setup_test_dir();
    fs::write(dir.path().join(".env"), "SECRET=value").unwrap();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/.env", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));
}

// --- .well-known PHP blocking tests ---

#[tokio::test]
async fn test_resolve_blocks_php_in_well_known() {
    let dir = setup_test_dir();
    fs::create_dir_all(dir.path().join(".well-known")).unwrap();
    fs::write(
        dir.path().join(".well-known/test.php"),
        "<?php echo 'hack';",
    )
    .unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/.well-known/test.php", &cache).await;
    assert!(
        matches!(*result, RouteResult::NotFound),
        "PHP in .well-known must not execute, got {:?}",
        result
    );
}

#[tokio::test]
async fn test_resolve_well_known_missing_file_in_framework_rewrites() {
    let dir = setup_test_dir();
    fs::create_dir_all(dir.path().join(".well-known")).unwrap();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc
        .resolve_request("/.well-known/openid-configuration", &cache)
        .await;
    // no extension → resolve_no_extension → rewrite to index.php
    match &*result {
        RouteResult::Execute(path, Some(pi), _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(pi, "/.well-known/openid-configuration");
        }
        other => panic!("Expected rewrite to index.php, got {:?}", other),
    }
}

#[tokio::test]
async fn test_resolve_well_known_missing_file_traditional_falls_back() {
    let dir = setup_test_dir();
    fs::create_dir_all(dir.path().join(".well-known")).unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc
        .resolve_request("/.well-known/openid-configuration", &cache)
        .await;
    // Traditional: no extension → resolve_no_extension → root fallback → index.php exists
    match &*result {
        RouteResult::Execute(path, _, _) => {
            assert!(path.ends_with("index.php"));
        }
        other => panic!("Expected fallback to index.php, got {:?}", other),
    }
}

// --- Unicode path tests (Cyrillic / CJK / Emoji) ---
//
// These tests verify that the byte-level hot-path optimisations
// (has_dot_segment_markers, already_sanitized, has_php_component,
// contains_blocked_dot_segment) stay correct for non-ASCII UTF-8 where
// every continuation byte is 0x80-0xBF and start bytes are 0xC2-0xF4 —
// none of which collide with the ASCII markers we scan for ('/', '.', '%').

#[test]
fn test_sanitize_path_preserves_cyrillic() {
    // `/страница` — raw Cyrillic in the already-decoded input.
    assert_eq!(sanitize_path("/страница"), "страница");
    assert_eq!(sanitize_path("/api/пользователи/42"), "api/пользователи/42");
}

#[test]
fn test_sanitize_path_preserves_cjk() {
    assert_eq!(sanitize_path("/文件/列表"), "文件/列表");
}

#[test]
fn test_sanitize_path_preserves_emoji() {
    assert_eq!(sanitize_path("/🎉/party"), "🎉/party");
    // Multi-codepoint emoji (family): 👨‍👩‍👧 uses ZWJ, still no '/' or '.'.
    assert_eq!(sanitize_path("/👨‍👩‍👧"), "👨‍👩‍👧");
}

#[test]
fn test_sanitize_path_unicode_still_removes_dotdot() {
    // Traversal must still be stripped even when surrounded by non-ASCII.
    assert_eq!(sanitize_path("/страница/../другая"), "страница/другая");
    assert_eq!(sanitize_path("/文件/./список"), "文件/список");
}

#[test]
fn test_has_dot_segment_markers_clean_cyrillic() {
    // Raw Cyrillic has no ASCII markers — must take the zero-alloc fast path.
    assert!(!has_dot_segment_markers("/страница"));
    assert!(!has_dot_segment_markers("/api/пользователи"));
}

#[test]
fn test_has_dot_segment_markers_clean_cjk_and_emoji() {
    assert!(!has_dot_segment_markers("/文件/列表"));
    assert!(!has_dot_segment_markers("/🎉/party"));
    assert!(!has_dot_segment_markers("/👨‍👩‍👧"));
}

#[test]
fn test_has_dot_segment_markers_triggers_on_percent_encoded_unicode() {
    // Percent-encoded UTF-8 contains '%' → full decode path must run so
    // we catch any %2e bypass hidden inside.
    assert!(has_dot_segment_markers(
        "/%D1%81%D1%82%D1%80%D0%B0%D0%BD%D0%B8%D1%86%D0%B0"
    ));
}

#[test]
fn test_classify_unicode_no_extension() {
    // Non-ASCII "extension" is not a valid extension — classified as no-ext.
    assert_eq!(classify_uri("отчёт.документ"), UriKind::NoExtension);
    assert_eq!(classify_uri("文件.列表"), UriKind::NoExtension);
    assert_eq!(classify_uri("party.🎉"), UriKind::NoExtension);
}

#[test]
fn test_classify_unicode_with_ascii_extension() {
    // Non-ASCII basename but ASCII extension — proper extension.
    assert_eq!(classify_uri("страница.html"), UriKind::OtherExtension);
    assert_eq!(classify_uri("文件.css"), UriKind::OtherExtension);
    assert_eq!(classify_uri("party/🎉.png"), UriKind::OtherExtension);
}

#[test]
fn test_has_php_component_ignores_cyrillic_lookalike() {
    // U+0420 CYRILLIC CAPITAL LETTER ER encodes as D0 A0 — must NOT match
    // ASCII `p` (0x70) under the `| 0x20` case-insensitive compare.
    assert!(!has_php_component("/hack.Рhp")); // first letter is Cyrillic Р
    assert!(!has_php_component("/admin.рhp")); // Cyrillic р
}

#[test]
fn test_has_php_component_true_in_unicode_path() {
    // Real `.php` after Unicode segments still matches.
    assert!(has_php_component("/страница/about.php"));
    assert!(has_php_component("/文件/index.php/user/42"));
    assert!(has_php_component("/🎉/handler.PHP"));
}

#[test]
fn test_contains_blocked_dot_segment_unicode_is_allowed() {
    // Non-ASCII segments must not be mistaken for dot-segments.
    assert!(!contains_blocked_dot_segment("/страница/файл"));
    assert!(!contains_blocked_dot_segment("/文件/列表"));
    assert!(!contains_blocked_dot_segment("/🎉"));
}

#[test]
fn test_contains_blocked_dot_segment_unicode_with_hidden_file() {
    // A `.hidden` segment after Unicode segments must still be blocked.
    assert!(contains_blocked_dot_segment("/страница/.hidden"));
    assert!(contains_blocked_dot_segment("/文件/.git/config"));
}

#[tokio::test]
async fn test_resolve_serves_file_with_cyrillic_name() {
    let dir = setup_test_dir();
    fs::write(dir.path().join("страница.html"), "<html>Ok</html>").unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));

    // Percent-encoded UTF-8 — the form browsers actually send.
    let result = rc
        .resolve_request(
            "/%D1%81%D1%82%D1%80%D0%B0%D0%BD%D0%B8%D1%86%D0%B0.html",
            &cache,
        )
        .await;
    match &*result {
        RouteResult::Serve(path) => assert!(path.ends_with("страница.html")),
        other => panic!("Expected Serve(страница.html), got {:?}", other),
    }
}

#[tokio::test]
async fn test_resolve_serves_file_with_emoji_name() {
    let dir = setup_test_dir();
    fs::write(dir.path().join("🎉.html"), "<html>Party</html>").unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));

    // Percent-encoded 🎉 (U+1F389) as UTF-8: F0 9F 8E 89.
    let result = rc.resolve_request("/%F0%9F%8E%89.html", &cache).await;
    match &*result {
        RouteResult::Serve(path) => assert!(path.ends_with("🎉.html")),
        other => panic!("Expected Serve(🎉.html), got {:?}", other),
    }
}

#[tokio::test]
async fn test_resolve_executes_php_with_cjk_path() {
    let dir = setup_test_dir();
    fs::create_dir_all(dir.path().join("文件")).unwrap();
    fs::write(dir.path().join("文件/index.php"), "<?php echo 'cjk';").unwrap();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));

    // /%E6%96%87%E4%BB%B6/index.php
    let result = rc
        .resolve_request("/%E6%96%87%E4%BB%B6/index.php", &cache)
        .await;
    match &*result {
        RouteResult::Execute(path, None, None) => {
            assert!(path.to_string_lossy().contains("文件"));
            assert!(path.ends_with("index.php"));
        }
        other => panic!("Expected Execute(文件/index.php), got {:?}", other),
    }
}

#[tokio::test]
async fn test_resolve_blocks_percent_encoded_traversal_in_unicode_path() {
    // An attacker tries to sneak `%2e%2e` (..) inside a Unicode-looking path.
    // `/страница/%2e%2e/secret` decodes to `/страница/../secret`; the `..`
    // segment starts with '.' → contains_blocked_dot_segment() blocks it.
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));

    let result = rc
        .resolve_request(
            "/%D1%81%D1%82%D1%80%D0%B0%D0%BD%D0%B8%D1%86%D0%B0/%2e%2e/secret",
            &cache,
        )
        .await;
    assert!(matches!(*result, RouteResult::NotFound));
}

#[tokio::test]
async fn test_resolve_blocks_percent_encoded_dotfile_after_unicode() {
    // `/страница/%2egit/HEAD` decodes to `/страница/.git/HEAD` and must
    // be blocked as a dot-segment.
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc
        .resolve_request(
            "/%D1%81%D1%82%D1%80%D0%B0%D0%BD%D0%B8%D1%86%D0%B0/%2egit/HEAD",
            &cache,
        )
        .await;
    assert!(matches!(*result, RouteResult::NotFound));
}

#[cfg(test)]
mod php_deny_integration {
    use super::*;
    use crate::config::ServerConfig;
    use crate::server::response::static_file::FileCache;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tempfile::TempDir;

    // Shared lock — the env-var-driven PhpDeny loader is process-global.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn setup(
        dir_layout: &[(&str, &[u8])],
        env: &[(&str, Option<&str>)],
        entry_file: Option<&str>,
    ) -> (TempDir, Arc<FileCache>, super::RouteConfig) {
        // Hold the lock for the full setup so env mutations don't race other tests.
        // Guard is dropped on function exit — by then RouteConfig has captured
        // whatever PhpDeny it needed, so the env can be safely restored.
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());

        let dir = TempDir::new().unwrap();
        for (rel, body) in dir_layout {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
        }

        // Save previous env, apply overrides.
        let prev: Vec<(String, Option<String>)> = env
            .iter()
            .map(|(k, _)| (k.to_string(), std::env::var(k).ok()))
            .collect();
        for (k, v) in env {
            match v {
                Some(val) => std::env::set_var(k, val),
                None => std::env::remove_var(k),
            }
        }

        let cfg = ServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            document_root: dir.path().to_path_buf(),
            header_read_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(120),
        };
        let entry_path = entry_file.map(|name| dir.path().join(name));
        let cache = Arc::new(FileCache::new(1024));
        let rc = super::RouteConfig::new(&cfg, entry_path.as_deref(), false);

        // Restore env so other tests aren't polluted.
        for (k, prev_val) in prev {
            match prev_val {
                Some(v) => std::env::set_var(&k, v),
                None => std::env::remove_var(&k),
            }
        }

        (dir, cache, rc)
    }

    #[tokio::test]
    async fn deny_status_blocks_uploaded_php() {
        let (_dir, cache, rc) = setup(
            &[("uploads/shell.php", b"<?php echo 'pwned';")],
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", None),
            ],
            None,
        );
        let result = rc.resolve_request("/uploads/shell.php", &cache).await;
        assert!(matches!(&*result, RouteResult::Denied(404)));
    }

    #[tokio::test]
    async fn deny_status_403_explicit() {
        let (_dir, cache, rc) = setup(
            &[("uploads/shell.php", b"<?php echo 'pwned';")],
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", Some("403")),
            ],
            None,
        );
        let result = rc.resolve_request("/uploads/shell.php", &cache).await;
        assert!(matches!(&*result, RouteResult::Denied(403)));
    }

    #[tokio::test]
    async fn deny_status_blocks_nonexistent_php_no_existence_oracle() {
        let (_dir, cache, rc) = setup(
            &[], // no file on disk
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", None),
            ],
            None,
        );
        let result = rc.resolve_request("/uploads/ghost.php", &cache).await;
        // Must be StatusCode (deny), not NotFound (would be an existence oracle).
        assert!(matches!(&*result, RouteResult::Denied(404)));
    }

    #[tokio::test]
    async fn deny_leaves_static_files_alone() {
        let (_dir, cache, rc) = setup(
            &[("uploads/image.png", b"\x89PNG")],
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", None),
            ],
            None,
        );
        let result = rc.resolve_request("/uploads/image.png", &cache).await;
        assert!(matches!(&*result, RouteResult::Serve(_)));
    }

    #[tokio::test]
    async fn deny_script_fallback_returns_execute_with_meta() {
        let (_dir, cache, rc) = setup(
            &[
                ("uploads/shell.php", b"<?php echo 'pwned';"),
                ("_security/denied.php", b"<?php http_response_code(404);"),
            ],
            &[
                ("PHP_DENY_DIRS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", Some("/_security/denied.php")),
            ],
            None,
        );
        let result = rc.resolve_request("/uploads/shell.php", &cache).await;
        match &*result {
            RouteResult::Execute(_, path_info, Some(meta)) => {
                // path_info is None on the deny-script path — the original
                // URI lives in `meta.path` only (no duplicate String alloc).
                assert!(path_info.is_none());
                assert_eq!(meta.path, "uploads/shell.php");
                assert_eq!(meta.pattern, "uploads/**");
                assert_eq!(meta.fallback_script_uri, "/_security/denied.php");
            }
            other => panic!("expected Execute with DeniedMeta, got {other:?}"),
        }
    }
}
