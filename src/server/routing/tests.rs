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

/// Construct a `RouteConfig` for tests with a guaranteed-empty
/// `SYMLINK_ALLOW_PATHS`. Routing tests that don't care about the
/// allow-list want to see strict default symlink-escape behaviour,
/// regardless of whether a concurrent test in the `symlink_allow`
/// module has the env var set inside its `with_env` window.
///
/// We acquire the shared `ENV_LOCK`, snapshot+clear the env var,
/// construct, then restore — same pattern as `php_deny::tests::with_env`.
/// Tests that *do* need the env var set (e.g.
/// `test_symlink_to_allowed_path_resolves`) build `RouteConfig` directly
/// inside their own `with_env` window; calling this helper from inside one
/// would deadlock.
fn make_config(dir: &Path, entry_file: Option<&str>) -> RouteConfig {
    build_config_clean_env(dir, entry_file.map(|name| dir.join(name)), false)
}

/// Shared builder behind `make_config` / `make_worker_config`: constructs a
/// `RouteConfig` with the env hygiene described above.
fn build_config_clean_env(
    dir: &Path,
    entry_path: Option<std::path::PathBuf>,
    worker_mode: bool,
) -> RouteConfig {
    crate::config::test_env::with_env(&[("SYMLINK_ALLOW_PATHS", None)], || {
        let config = ServerConfig::new("0.0.0.0:8080".to_string(), dir.to_path_buf());
        RouteConfig::new(&config, entry_path.as_deref(), worker_mode)
    })
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
async fn test_traditional_missing_php_prefix_yields_no_path_info() {
    // `/cvdvfdv.php/dwvrb` where `cvdvfdv.php` does NOT exist on disk.
    // try_split_path_info finds the `.php` marker but the prefix script is
    // absent, so it gives up and the request falls through root_fallback to
    // index.php with `path_info = None`. The original `/cvdvfdv.php/dwvrb` is
    // therefore NOT surfaced as `$_SERVER['PATH_INFO']` — the front
    // controller must read `REQUEST_URI` instead. This mirrors stock
    // nginx + php-fpm, where a `try_files … /index.php` rewrite also leaves
    // PATH_INFO unset.
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), None);
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/cvdvfdv.php/dwvrb", &cache).await;
    match &*result {
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("index.php"), "got {:?}", path);
            assert_eq!(
                *path_info, None,
                "missing-prefix fallback must not carry PATH_INFO"
            );
        }
        other => panic!("Expected fallback Execute(index.php), got {:?}", other),
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
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(*path_info, None, "app route carries no PATH_INFO");
        }
        other => panic!("Expected Execute(index.php), got {:?}", other),
    }
}

#[tokio::test]
async fn test_framework_root_goes_to_index_php_no_path_info() {
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/", &cache).await;
    match &*result {
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(*path_info, None, "root carries no PATH_INFO");
        }
        other => panic!("Expected Execute(index.php), got {:?}", other),
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
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(*path_info, None, "non-entry .php carries no PATH_INFO");
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
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(*path_info, None, "bare entry carries no PATH_INFO");
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
async fn test_framework_missing_static_falls_back_to_entry() {
    // Non-.php extension with no file on disk → fall back to the front
    // controller (`try_files $uri /index.php`); original URI in PATH_INFO.
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/missing.png", &cache).await;
    match &*result {
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(*path_info, None, "static fallback carries no PATH_INFO");
        }
        other => panic!("expected Execute(index.php), got {other:?}"),
    }
}

#[tokio::test]
async fn test_framework_missing_static_hard_404_when_entry_missing() {
    // If the front controller itself is absent, a static miss has nothing to
    // fall back to → hard 404 (no worker route configured).
    let dir = setup_test_dir();
    fs::remove_file(dir.path().join("index.php")).unwrap();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/missing.png", &cache).await;
    assert!(matches!(*result, RouteResult::NotFound));
}

#[tokio::test]
async fn test_framework_non_entry_php_path_no_path_info() {
    // /api.php/v1/users → rewrite to index.php. api.php is NOT the entry file,
    // so the request does not explicitly name the front controller → no PATH_INFO.
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/api.php/v1/users", &cache).await;
    match &*result {
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(*path_info, None);
        }
        other => panic!("Expected index.php rewrite, got {:?}", other),
    }
}

#[tokio::test]
async fn test_framework_explicit_entry_sets_path_info() {
    // /index.php/news/local explicitly names the front controller → honest
    // CGI PATH_INFO = the trailing segment.
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/index.php/news/local", &cache).await;
    match &*result {
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(path_info.as_deref(), Some("/news/local"));
        }
        other => panic!("Expected Execute(index.php, /news/local), got {:?}", other),
    }
}

#[tokio::test]
async fn test_framework_entry_prefix_not_a_segment_no_path_info() {
    // /index.phpfoo must NOT match the entry prefix (next char is not `/`).
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/index.phpfoo", &cache).await;
    match &*result {
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(*path_info, None);
        }
        other => panic!("Expected Execute(index.php), got {:?}", other),
    }
}

#[tokio::test]
async fn test_framework_entry_trailing_slash_no_path_info() {
    // /index.php/ — `sanitize` collapses the empty trailing segment to
    // `index.php`, so there is no segment left to expose. Locked behavior: no
    // PATH_INFO (a trailing-slash redirect is left to the application).
    let dir = setup_test_dir();
    let rc = make_config(dir.path(), Some("index.php"));
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/index.php/", &cache).await;
    match &*result {
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(*path_info, None);
        }
        other => panic!("Expected Execute(index.php), got {:?}", other),
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

// --- Worker mode ---

/// Build a worker-mode `RouteConfig` with the worker entry wired up.
/// Same `SYMLINK_ALLOW_PATHS` hygiene as `make_config`. The worker script
/// must already exist under `dir`.
fn make_worker_config(dir: &Path, worker_file: &str) -> RouteConfig {
    let worker_path = dir.join(worker_file);
    let mut rc = build_config_clean_env(dir, Some(worker_path.clone()), true);
    rc.set_worker_route(worker_path);
    rc
}

fn setup_worker_dir() -> TempDir {
    let dir = setup_test_dir();
    fs::write(dir.path().join("worker.php"), "<?php // worker bootstrap").unwrap();
    dir
}

fn assert_worker_execute(result: &RouteResult) {
    match result {
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("worker.php"), "got {path:?}");
            assert_eq!(*path_info, None, "worker route must not carry PATH_INFO");
        }
        other => panic!("expected worker Execute, got {other:?}"),
    }
}

#[tokio::test]
async fn test_worker_existing_php_dispatches_to_worker() {
    // about.php exists on disk, but worker mode never executes arbitrary
    // .php files per-request — the worker is the single front controller.
    let dir = setup_worker_dir();
    let rc = make_worker_config(dir.path(), "worker.php");
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/about.php", &cache).await;
    assert_worker_execute(&result);
}

#[tokio::test]
async fn test_worker_root_index_php_does_not_absorb() {
    // Root index.php exists, yet `/` and unmatched routes still go to the
    // worker — the Traditional root fallback must not absorb requests
    // before the worker sees them.
    let dir = setup_worker_dir();
    let rc = make_worker_config(dir.path(), "worker.php");
    let cache = Arc::new(FileCache::new(200));
    assert_worker_execute(&*rc.resolve_request("/", &cache).await);
    assert_worker_execute(&*rc.resolve_request("/api/users", &cache).await);
}

#[tokio::test]
async fn test_worker_directory_index_goes_to_worker() {
    // `/blog/` with blog/index.php on disk → worker, not a per-request
    // execution of the directory index.
    let dir = setup_worker_dir();
    fs::create_dir_all(dir.path().join("blog")).unwrap();
    fs::write(dir.path().join("blog/index.php"), "<?php echo 'blog';").unwrap();
    let rc = make_worker_config(dir.path(), "worker.php");
    let cache = Arc::new(FileCache::new(200));
    assert_worker_execute(&*rc.resolve_request("/blog/", &cache).await);
}

#[tokio::test]
async fn test_worker_static_file_served() {
    let dir = setup_worker_dir();
    let rc = make_worker_config(dir.path(), "worker.php");
    let cache = Arc::new(FileCache::new(200));
    let result = rc.resolve_request("/style.css", &cache).await;
    match &*result {
        RouteResult::Serve(path) => assert!(path.ends_with("style.css")),
        other => panic!("expected Serve, got {other:?}"),
    }
}

#[tokio::test]
async fn test_worker_static_miss_goes_to_worker() {
    // Missing assets fall through to the worker, not a hard 404.
    let dir = setup_worker_dir();
    let rc = make_worker_config(dir.path(), "worker.php");
    let cache = Arc::new(FileCache::new(200));
    assert_worker_execute(&*rc.resolve_request("/missing.png", &cache).await);
}

#[tokio::test]
async fn test_worker_php_path_info_goes_to_worker() {
    // `.php/extra` URIs do not PATH_INFO-split in worker mode — straight
    // to the worker, no per-request script resolution.
    let dir = setup_worker_dir();
    let rc = make_worker_config(dir.path(), "worker.php");
    let cache = Arc::new(FileCache::new(200));
    assert_worker_execute(&*rc.resolve_request("/about.php/v1/users", &cache).await);
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

#[cfg(unix)]
#[tokio::test]
async fn test_symlink_to_allowed_path_resolves() {
    use crate::config::symlink_allow::tests::non_blacklisted_tempdir;
    use std::os::unix::fs::symlink;

    let dir = setup_test_dir();
    // Use non_blacklisted_tempdir() so the symlink target's canonical path
    // doesn't land under a BLACKLIST_PREFIXES entry on Linux CI.
    let target = non_blacklisted_tempdir();
    let target_canonical = std::fs::canonicalize(target.path()).unwrap();
    fs::write(target_canonical.join("asset.txt"), "allowed asset").unwrap();
    symlink(target.path(), dir.path().join("assets")).unwrap();

    // Set SYMLINK_ALLOW_PATHS only across RouteConfig::new — it captures
    // the allow-list into the struct, after which env mutation is irrelevant.
    // Closing the env window before the .await avoids
    // clippy::await_holding_lock on its std::sync::Mutex guard. Build
    // RouteConfig directly because make_config() opens its own env window
    // that clears SYMLINK_ALLOW_PATHS.
    let allow = [(
        "SYMLINK_ALLOW_PATHS",
        Some(target_canonical.to_str().unwrap()),
    )];
    let rc = crate::config::test_env::with_env(&allow, || {
        let server_config = ServerConfig::new("0.0.0.0:8080".to_string(), dir.path().to_path_buf());
        RouteConfig::new(&server_config, None, false)
    });
    let cache = Arc::new(FileCache::new(200));
    let res = rc.resolve_request("/assets/asset.txt", &cache).await;
    assert!(
        matches!(*res, RouteResult::Serve(_)),
        "expected Serve for allow-listed symlink, got {:?}",
        *res
    );
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
    // no extension → resolve_no_extension → rewrite to index.php.
    // App route (does not name the entry file) → no PATH_INFO.
    match &*result {
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("index.php"));
            assert_eq!(*path_info, None);
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

#[tokio::test]
async fn test_worker_mode_route_never_carries_path_info() {
    // Worker mode routes every non-static request to the persistent worker
    // script via `set_worker_route`, which is hardcoded to `path_info = None`
    // (see RouteConfig::set_worker_route). A worker therefore never receives
    // `$_SERVER['PATH_INFO']` — not even the original URI — and must dispatch
    // on `REQUEST_URI`. This test pins that behaviour.
    let dir = TempDir::new().unwrap();
    let worker = dir.path().join("worker.php");
    fs::write(&worker, "<?php echo 'worker';").unwrap();

    // Build the config inside an env window so its lock guard is dropped
    // before any `.await` below (clippy::await_holding_lock). The window only
    // needs to cover `RouteConfig::new`, which reads `SYMLINK_ALLOW_PATHS`.
    let mut rc = crate::config::test_env::with_env(&[("SYMLINK_ALLOW_PATHS", None)], || {
        let config = ServerConfig::new("0.0.0.0:8080".to_string(), dir.path().to_path_buf());
        RouteConfig::new(&config, Some(worker.as_path()), true)
    });
    rc.set_worker_route(worker.clone());

    let cache = Arc::new(FileCache::new(200));

    // Plain unmatched path.
    let result = rc.resolve_request("/some/random/path", &cache).await;
    match &*result {
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("worker.php"), "got {:?}", path);
            assert_eq!(*path_info, None, "worker route must not carry PATH_INFO");
        }
        other => panic!("Expected worker Execute, got {:?}", other),
    }

    // Even a `.php/extra` URI (which would split in plain Traditional mode)
    // carries no PATH_INFO once the prefix is missing and it lands on the
    // worker.
    let result = rc.resolve_request("/api.php/v1/users", &cache).await;
    match &*result {
        RouteResult::Execute(path, path_info, _) => {
            assert!(path.ends_with("worker.php"), "got {:?}", path);
            assert_eq!(*path_info, None, "worker route must not carry PATH_INFO");
        }
        other => panic!("Expected worker Execute, got {:?}", other),
    }
}

#[cfg(test)]
mod php_deny_integration {
    use super::*;
    use crate::config::{DenySource, ServerConfig};
    use crate::server::response::static_file::FileCache;
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;

    fn setup(
        dir_layout: &[(&str, &[u8])],
        env: &[(&str, Option<&str>)],
        entry_file: Option<&str>,
    ) -> (TempDir, Arc<FileCache>, super::RouteConfig) {
        setup_mode(dir_layout, env, entry_file, false)
    }

    fn setup_mode(
        dir_layout: &[(&str, &[u8])],
        env: &[(&str, Option<&str>)],
        entry_file: Option<&str>,
        worker_mode: bool,
    ) -> (TempDir, Arc<FileCache>, super::RouteConfig) {
        let dir = TempDir::new().unwrap();
        for (rel, body) in dir_layout {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
        }

        let cfg = ServerConfig {
            listen_addr: "127.0.0.1:0".to_string(),
            document_root: dir.path().to_path_buf(),
            header_read_timeout: Duration::from_secs(5),
            deny_file: None,
        };
        let entry_path = entry_file.map(|name| dir.path().join(name));
        let cache = Arc::new(FileCache::new(1024));
        // Env mutations are scoped (and panic-safe) via the shared harness,
        // held only across RouteConfig construction — it captures whatever
        // PhpDeny it needs, so the env is restored as soon as it returns.
        let rc = crate::config::test_env::with_env(env, || {
            let mut rc = super::RouteConfig::new(&cfg, entry_path.as_deref(), worker_mode);
            if worker_mode {
                rc.set_worker_route(entry_path.clone().expect("worker mode requires entry"));
            }
            rc
        });

        (dir, cache, rc)
    }

    #[tokio::test]
    async fn deny_status_blocks_uploaded_php() {
        let (_dir, cache, rc) = setup(
            &[("uploads/shell.php", b"<?php echo 'pwned';")],
            &[
                ("PHP_DENY_PATHS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", None),
            ],
            None,
        );
        let result = rc.resolve_request("/uploads/shell.php", &cache).await;
        assert!(matches!(
            &*result,
            RouteResult::Denied {
                status: 404,
                source: DenySource::PhpDenyPaths
            }
        ));
    }

    #[tokio::test]
    async fn deny_status_403_explicit() {
        let (_dir, cache, rc) = setup(
            &[("uploads/shell.php", b"<?php echo 'pwned';")],
            &[
                ("PHP_DENY_PATHS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", Some("403")),
            ],
            None,
        );
        let result = rc.resolve_request("/uploads/shell.php", &cache).await;
        assert!(matches!(
            &*result,
            RouteResult::Denied {
                status: 403,
                source: DenySource::PhpDenyPaths
            }
        ));
    }

    #[tokio::test]
    async fn deny_status_blocks_nonexistent_php_no_existence_oracle() {
        let (_dir, cache, rc) = setup(
            &[], // no file on disk
            &[
                ("PHP_DENY_PATHS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", None),
            ],
            None,
        );
        let result = rc.resolve_request("/uploads/ghost.php", &cache).await;
        // Must be StatusCode (deny), not NotFound (would be an existence oracle).
        assert!(matches!(
            &*result,
            RouteResult::Denied {
                status: 404,
                source: DenySource::PhpDenyPaths
            }
        ));
    }

    #[tokio::test]
    async fn deny_leaves_static_files_alone() {
        let (_dir, cache, rc) = setup(
            &[("uploads/image.png", b"\x89PNG")],
            &[
                ("PHP_DENY_PATHS", Some("/uploads/**")),
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
                ("PHP_DENY_PATHS", Some("/uploads/**")),
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
                assert_eq!(meta.source, DenySource::PhpDenyPaths);
            }
            other => panic!("expected Execute with DeniedMeta, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn deny_path_info_split_caught_by_resolved_path_screen() {
        // Single-star pattern: the full URI `uploads/shell.php/x` does NOT
        // match `/uploads/*.php` (literal separator), so the pre-dispatch
        // screen passes. The PATH_INFO split resolves the script part to
        // `uploads/shell.php`, which the post-dispatch resolved-path screen
        // must deny.
        let (_dir, cache, rc) = setup(
            &[("uploads/shell.php", b"<?php echo 'pwned';")],
            &[
                ("PHP_DENY_PATHS", Some("/uploads/*.php")),
                ("PHP_DENY_FALLBACK", None),
            ],
            None,
        );
        let result = rc.resolve_request("/uploads/shell.php/x", &cache).await;
        assert!(matches!(
            &*result,
            RouteResult::Denied {
                status: 404,
                source: DenySource::PhpDenyPaths
            }
        ));
    }

    #[tokio::test]
    async fn deny_spa_blocks_uploaded_php() {
        // SPA mode maps .php URIs directly to disk — the deny-list applies.
        let (_dir, cache, rc) = setup(
            &[
                ("index.html", b"<html></html>"),
                ("uploads/shell.php", b"<?php echo 'pwned';"),
            ],
            &[
                ("PHP_DENY_PATHS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", None),
            ],
            Some("index.html"),
        );
        let result = rc.resolve_request("/uploads/shell.php", &cache).await;
        assert!(matches!(
            &*result,
            RouteResult::Denied {
                status: 404,
                source: DenySource::PhpDenyPaths
            }
        ));
    }

    #[tokio::test]
    async fn deny_spa_leaves_other_php_alone() {
        let (_dir, cache, rc) = setup(
            &[
                ("index.html", b"<html></html>"),
                ("api.php", b"<?php echo 'api';"),
            ],
            &[
                ("PHP_DENY_PATHS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", None),
            ],
            Some("index.html"),
        );
        let result = rc.resolve_request("/api.php", &cache).await;
        match &*result {
            RouteResult::Execute(path, _, None) => assert!(path.ends_with("api.php")),
            other => panic!("expected Execute, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn deny_directory_index_blocked() {
        // `/uploads/` resolves to `uploads/index.php` via the directory-index
        // lookup — a non-.php URI the pre-dispatch screen cannot see. The
        // post-dispatch screen must catch the resolved script path.
        let (_dir, cache, rc) = setup(
            &[("uploads/index.php", b"<?php echo 'pwned';")],
            &[
                ("PHP_DENY_PATHS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", None),
            ],
            None,
        );
        let result = rc.resolve_request("/uploads/", &cache).await;
        assert!(matches!(
            &*result,
            RouteResult::Denied {
                status: 404,
                source: DenySource::PhpDenyPaths
            }
        ));
    }

    #[tokio::test]
    async fn deny_directory_index_script_fallback_carries_meta() {
        let (_dir, cache, rc) = setup(
            &[
                ("uploads/index.php", b"<?php echo 'pwned';"),
                ("_security/denied.php", b"<?php http_response_code(404);"),
            ],
            &[
                ("PHP_DENY_PATHS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", Some("/_security/denied.php")),
            ],
            None,
        );
        let result = rc.resolve_request("/uploads/", &cache).await;
        match &*result {
            RouteResult::Execute(_, _, Some(meta)) => {
                // `meta.path` keeps the original request URI, not the
                // resolved directory-index path.
                assert_eq!(meta.path, "uploads");
                assert_eq!(meta.pattern, "uploads/**");
                assert_eq!(meta.source, DenySource::PhpDenyPaths);
            }
            other => panic!("expected Execute with DeniedMeta, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn deny_framework_mode_ignored() {
        // Framework mode: deny-list is disabled at startup; .php URIs route
        // to the front controller as application routes.
        let (_dir, cache, rc) = setup(
            &[("index.php", b"<?php echo 'app';")],
            &[
                ("PHP_DENY_PATHS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", None),
            ],
            Some("index.php"),
        );
        let result = rc.resolve_request("/uploads/shell.php", &cache).await;
        match &*result {
            RouteResult::Execute(path, _, None) => assert!(path.ends_with("index.php")),
            other => panic!("expected front-controller Execute, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn deny_worker_mode_ignored() {
        // Worker mode: deny-list is disabled at startup; .php URIs dispatch
        // to the worker and are never executed directly anyway.
        let (_dir, cache, rc) = setup_mode(
            &[
                ("worker.php", b"<?php // worker bootstrap"),
                ("uploads/shell.php", b"<?php echo 'pwned';"),
            ],
            &[
                ("PHP_DENY_PATHS", Some("/uploads/**")),
                ("PHP_DENY_FALLBACK", None),
            ],
            Some("worker.php"),
            true,
        );
        let result = rc.resolve_request("/uploads/shell.php", &cache).await;
        match &*result {
            RouteResult::Execute(path, _, None) => assert!(path.ends_with("worker.php")),
            other => panic!("expected worker Execute, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod deny_file_integration {
    use super::*;
    use crate::config::{DenyFile, DenySource, RoutingModeKind, ServerConfig};
    use crate::server::response::static_file::FileCache;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// A document root holding `layout` and, when `rules` is given, a
    /// `.oxphpdeny` with them, routed as `entry_file` and `worker_mode`
    /// select. `env` applies while the file and the routing are built; the
    /// deny variables are cleared first so an inherited value cannot leak in.
    fn setup(
        layout: &[(&str, &[u8])],
        rules: Option<&str>,
        env: &[(&str, Option<&str>)],
        entry_file: Option<&str>,
        worker_mode: bool,
    ) -> (TempDir, Arc<FileCache>, super::RouteConfig) {
        let dir = TempDir::new().unwrap();
        for (rel, body) in layout {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, body).unwrap();
        }
        if let Some(rules) = rules {
            std::fs::write(dir.path().join(".oxphpdeny"), rules).unwrap();
        }
        let entry_path = entry_file.map(|name| dir.path().join(name));
        let mut vars: Vec<(&str, Option<&str>)> = vec![
            ("PHP_DENY_PATHS", None),
            ("PHP_DENY_DIRS", None),
            ("PHP_DENY_FALLBACK", None),
            ("SYMLINK_ALLOW_PATHS", None),
        ];
        vars.extend_from_slice(env);
        let rc = crate::config::test_env::with_env(&vars, || {
            let mut cfg = ServerConfig::new("127.0.0.1:0".to_string(), dir.path().to_path_buf());
            let mode = RoutingModeKind::resolve(entry_path.as_deref(), worker_mode);
            cfg.deny_file = DenyFile::load(dir.path(), mode).unwrap().map(Arc::new);
            let mut rc = super::RouteConfig::new(&cfg, entry_path.as_deref(), worker_mode);
            if worker_mode {
                rc.set_worker_route(entry_path.clone().expect("worker mode requires entry"));
            }
            rc
        });
        (dir, Arc::new(FileCache::new(1024)), rc)
    }

    fn is_denied(result: &RouteResult, status: u16) -> bool {
        matches!(
            result,
            RouteResult::Denied { status: s, source: DenySource::DenyFile } if *s == status
        )
    }

    fn cached(rc: &super::RouteConfig, uri: &str) -> bool {
        rc.route_cache.lock().unwrap().peek(uri).is_some()
    }

    #[tokio::test]
    async fn deny_rules_apply_in_every_mode_whether_or_not_the_file_exists() {
        let layout: &[(&str, &[u8])] = &[
            ("index.php", b"<?php"),
            ("index.html", b"<html>"),
            ("vendor/autoload.php", b"<?php"),
            ("vendor/lib.js", b"js"),
            ("dump.sql", b"--"),
        ];
        for (entry, worker) in [
            (None, false),
            (Some("index.php"), false),
            (Some("index.html"), false),
            (Some("index.php"), true),
        ] {
            let (_dir, cache, rc) = setup(layout, Some("vendor/\n*.sql\n"), &[], entry, worker);
            for uri in [
                "/vendor/autoload.php",
                "/vendor/ghost.php",
                "/vendor/lib.js",
                "/vendor/ghost.js",
                "/vendor/",
                "/dump.sql",
                "/ghost.sql",
            ] {
                let result = rc.resolve_request(uri, &cache).await;
                assert!(
                    is_denied(&result, 404),
                    "entry {entry:?}, worker {worker}, {uri}: {result:?}"
                );
                assert!(!cached(&rc, uri), "{uri}: a denial was cached");
            }
        }
    }

    #[tokio::test]
    async fn paths_the_rules_do_not_name_route_as_before() {
        let (_dir, cache, rc) = setup(
            &[("index.php", b"<?php"), ("assets/app.js", b"js")],
            Some("vendor/\n"),
            &[],
            None,
            false,
        );
        assert!(matches!(
            &*rc.resolve_request("/assets/app.js", &cache).await,
            RouteResult::Serve(_)
        ));
        // `/` has no path for a rule to name.
        assert!(matches!(
            &*rc.resolve_request("/", &cache).await,
            RouteResult::Execute(_, _, None)
        ));
    }

    #[tokio::test]
    async fn normalization_does_not_bypass_the_rules() {
        let (_dir, cache, rc) = setup(
            &[("vendor/x.js", b"js")],
            Some("vendor/\n"),
            &[],
            None,
            false,
        );
        for uri in [
            "//vendor/x.js",
            "/vendor//x.js",
            "/vend%6Fr/x.js",
            "/vendor%2Fx.js",
        ] {
            let result = rc.resolve_request(uri, &cache).await;
            assert!(is_denied(&result, 404), "{uri}: {result:?}");
        }
        // Dot segments are refused before the rules run.
        for uri in ["/./vendor/x.js", "/a/../vendor/x.js"] {
            let result = rc.resolve_request(uri, &cache).await;
            assert!(
                matches!(&*result, RouteResult::NotFound),
                "{uri}: {result:?}"
            );
        }
        // A deny rule ignores case: a case-insensitive filesystem would
        // serve `vendor/x.js` for this.
        let result = rc.resolve_request("/VENDOR/x.js", &cache).await;
        assert!(is_denied(&result, 404), "{result:?}");
    }

    #[tokio::test]
    async fn trailing_slash_table() {
        // `admin/` misses the bare `/admin`; `admin` hits all four; `admin/*`
        // and `admin/x` only what lies below.
        let uris = ["/admin", "/admin/", "/admin/x", "/admin/x/y"];
        for (rules, want) in [
            ("admin/", [false, true, true, true]),
            ("admin", [true, true, true, true]),
            ("/admin", [true, true, true, true]),
            ("admin/*", [false, false, true, true]),
            ("admin/x", [false, false, true, true]),
        ] {
            let (_dir, cache, rc) = setup(
                &[("admin/index.html", b"<html>")],
                Some(rules),
                &[],
                None,
                false,
            );
            for (uri, want) in uris.iter().zip(want) {
                let result = rc.resolve_request(uri, &cache).await;
                assert_eq!(
                    is_denied(&result, 404),
                    want,
                    "rule {rules:?}, {uri}: {result:?}"
                );
            }
        }
        // Unanchored names match at any depth, anchored ones at the top only.
        let (_dir, cache, rc) = setup(&[], Some("admin\n"), &[], None, false);
        assert!(is_denied(
            &*rc.resolve_request("/foo/admin", &cache).await,
            404
        ));
        let (_dir, cache, rc) = setup(&[], Some("/admin\n"), &[], None, false);
        assert!(!is_denied(
            &*rc.resolve_request("/foo/admin", &cache).await,
            404
        ));
    }

    #[tokio::test]
    async fn trailing_slash_variants_count_as_directories() {
        let (_dir, cache, rc) = setup(
            &[("admin/index.html", b"<html>"), ("composer.json", b"{}")],
            Some("admin/\n/composer.*\n"),
            &[],
            None,
            false,
        );
        for uri in ["/admin/", "/admin%2F", "/admin//", "/composer.json/"] {
            let result = rc.resolve_request(uri, &cache).await;
            assert!(is_denied(&result, 404), "{uri}: {result:?}");
        }
        assert!(matches!(
            &*rc.resolve_request("/admin/.", &cache).await,
            RouteResult::NotFound
        ));
    }

    #[tokio::test]
    async fn uri_rules_do_not_see_the_script_a_uri_resolves_to() {
        // The documented gap, pinned so that closing it is a decision. Rules
        // match the URI: `/admin` is not a directory request, so `admin/`
        // misses it — and Traditional mode answers it with `admin/index.php`.
        // A rule without the slash (`/admin`) closes both.
        let (_dir, cache, rc) = setup(
            &[("admin/index.php", b"<?php")],
            Some("admin/\n"),
            &[],
            None,
            false,
        );
        match &*rc.resolve_request("/admin", &cache).await {
            RouteResult::Execute(path, _, None) => {
                assert!(path.ends_with("admin/index.php"), "{path:?}")
            }
            other => panic!("expected admin/index.php to run, got {other:?}"),
        }
        // A file mask misses the directory index it resolves to.
        let (_dir, cache, rc) = setup(
            &[("uploads/index.php", b"<?php")],
            Some("uploads/*.php\n"),
            &[],
            None,
            false,
        );
        match &*rc.resolve_request("/uploads/", &cache).await {
            RouteResult::Execute(path, _, None) => {
                assert!(path.ends_with("uploads/index.php"), "{path:?}")
            }
            other => panic!("expected uploads/index.php to run, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn entry_rules_hand_the_request_to_the_front_controller() {
        let (dir, cache, rc) = setup(
            &[("index.php", b"<?php"), ("storage/invoice.pdf", b"%PDF")],
            Some("> storage/\n"),
            &[],
            Some("index.php"),
            false,
        );
        for uri in [
            "/storage/invoice.pdf",
            "/storage/ghost.pdf",
            "/storage/report",
        ] {
            match &*rc.resolve_request(uri, &cache).await {
                RouteResult::Execute(path, None, None) => {
                    assert_eq!(path, &dir.path().join("index.php"), "{uri}")
                }
                other => panic!("{uri}: expected the front controller, got {other:?}"),
            }
            // Routing, not a denial: cached like any other route.
            assert!(cached(&rc, uri), "{uri} was not cached");
        }
    }

    #[tokio::test]
    async fn a_deny_below_an_entry_directory_still_denies() {
        // `>` hands a directory to the front controller, which may stream
        // its files to any logged-in user: a file the operator denied below
        // it must stay denied, whichever line comes first.
        for rules in [
            "*.sql\n> /storage/invoices/\n",
            "> /storage/invoices/\n*.sql\n",
        ] {
            let (_dir, cache, rc) = setup(
                &[
                    ("index.php", b"<?php"),
                    ("storage/invoices/dump.sql", b"--"),
                ],
                Some(rules),
                &[],
                Some("index.php"),
                false,
            );
            let result = rc
                .resolve_request("/storage/invoices/dump.sql", &cache)
                .await;
            assert!(is_denied(&result, 404), "{rules:?}: {result:?}");
        }
        let (_dir, cache, rc) = setup(
            &[("index.php", b"<?php"), ("storage/private/key.pem", b"k")],
            Some("> /storage/\n/storage/private/\n"),
            &[],
            Some("index.php"),
            false,
        );
        let result = rc.resolve_request("/storage/private/key.pem", &cache).await;
        assert!(is_denied(&result, 404), "{result:?}");
    }

    #[tokio::test]
    async fn entry_rules_in_worker_mode_dispatch_to_the_worker() {
        let (dir, cache, rc) = setup(
            &[("worker.php", b"<?php"), ("storage/invoice.pdf", b"%PDF")],
            Some("> storage/\n"),
            &[],
            Some("worker.php"),
            true,
        );
        match &*rc.resolve_request("/storage/invoice.pdf", &cache).await {
            RouteResult::Execute(path, None, None) => {
                assert_eq!(path, &dir.path().join("worker.php"))
            }
            other => panic!("expected the worker entry, got {other:?}"),
        }
    }

    #[test]
    #[should_panic(expected = "worker mode")]
    fn worker_mode_rejects_a_script_fallback_for_deny_rules() {
        let _ = setup(
            &[("worker.php", b"<?php"), ("_security/denied.php", b"<?php")],
            Some("vendor/\n"),
            &[("PHP_DENY_FALLBACK", Some("/_security/denied.php"))],
            Some("worker.php"),
            true,
        );
    }

    #[test]
    fn worker_mode_ignores_a_script_fallback_no_rule_can_use() {
        // `>` and `!` rules never answer with the fallback.
        let _ = setup(
            &[("worker.php", b"<?php"), ("_security/denied.php", b"<?php")],
            Some("> storage/\n!keep\n"),
            &[("PHP_DENY_FALLBACK", Some("/_security/denied.php"))],
            Some("worker.php"),
            true,
        );
    }

    #[tokio::test]
    async fn script_fallback_reports_the_rule_as_written() {
        let (_dir, cache, rc) = setup(
            &[("_security/denied.php", b"<?php"), ("secret.txt", b"s")],
            Some("/secret.*\n"),
            &[("PHP_DENY_FALLBACK", Some("/_security/denied.php"))],
            None,
            false,
        );
        match &*rc.resolve_request("/secret.txt", &cache).await {
            RouteResult::Execute(path, None, Some(meta)) => {
                assert!(path.ends_with("_security/denied.php"), "{path:?}");
                assert_eq!(meta.source, DenySource::DenyFile);
                assert_eq!(meta.path, "secret.txt");
                assert_eq!(meta.pattern, "/secret.*");
                assert_eq!(meta.fallback_script_uri, "/_security/denied.php");
            }
            other => panic!("expected the fallback script, got {other:?}"),
        }
        assert!(!cached(&rc, "/secret.txt"));
    }

    #[tokio::test]
    async fn fallback_script_may_hide_itself() {
        // `/_security/` keeps the script from being requested directly. Such a
        // request is denied and answered by the script — once, not in a loop:
        // a fallback script is run directly, never routed.
        let (_dir, cache, rc) = setup(
            &[("_security/denied.php", b"<?php")],
            Some("/_security/\n"),
            &[("PHP_DENY_FALLBACK", Some("/_security/denied.php"))],
            None,
            false,
        );
        match &*rc.resolve_request("/_security/denied.php", &cache).await {
            RouteResult::Execute(path, None, Some(meta)) => {
                assert!(path.ends_with("_security/denied.php"), "{path:?}");
                assert_eq!(meta.pattern, "/_security/");
            }
            other => panic!("expected the fallback script, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn deny_file_comes_first_and_shares_php_deny_paths_fallback() {
        let layout: &[(&str, &[u8])] =
            &[("uploads/shell.php", b"<?php"), ("uploads/a.png", b"png")];
        let env = &[
            ("PHP_DENY_PATHS", Some("/uploads/**")),
            ("PHP_DENY_FALLBACK", Some("403")),
        ];
        let (_dir, cache, rc) = setup(layout, Some("uploads/\n"), env, None, false);
        for uri in ["/uploads/shell.php", "/uploads/a.png"] {
            let result = rc.resolve_request(uri, &cache).await;
            assert!(is_denied(&result, 403), "{uri}: {result:?}");
        }
        // What the file does not name, `PHP_DENY_PATHS` still denies.
        let (_dir, cache, rc) = setup(layout, Some("vendor/\n"), env, None, false);
        assert!(matches!(
            &*rc.resolve_request("/uploads/shell.php", &cache).await,
            RouteResult::Denied {
                status: 403,
                source: DenySource::PhpDenyPaths
            }
        ));
    }

    #[test]
    #[should_panic(expected = "PHP_DENY_FALLBACK")]
    fn a_bad_fallback_stops_startup_when_a_rule_denies() {
        let _ = setup(
            &[],
            Some("vendor/\n"),
            &[("PHP_DENY_FALLBACK", Some("banana"))],
            None,
            false,
        );
    }

    #[tokio::test]
    async fn a_bad_fallback_is_not_read_when_no_rule_denies() {
        let (_dir, cache, rc) = setup(
            &[("index.php", b"<?php"), ("storage/a.pdf", b"%PDF")],
            Some("> storage/\n"),
            &[("PHP_DENY_FALLBACK", Some("banana"))],
            Some("index.php"),
            false,
        );
        assert!(matches!(
            &*rc.resolve_request("/storage/a.pdf", &cache).await,
            RouteResult::Execute(_, None, None)
        ));
    }

    #[tokio::test]
    async fn allowlist_file_serves_only_its_exceptions() {
        let (_dir, cache, rc) = setup(
            &[
                ("index.php", b"<?php"),
                ("assets/app.js", b"js"),
                ("lib/secret.php", b"<?php"),
                ("notes.txt", b"n"),
            ],
            Some("*\n!/index.php\n!/assets/\n!/assets/**\n"),
            &[],
            None,
            false,
        );
        // `/` has no path a rule can name: the root index still runs.
        assert!(matches!(
            &*rc.resolve_request("/", &cache).await,
            RouteResult::Execute(_, _, None)
        ));
        assert!(matches!(
            &*rc.resolve_request("/index.php", &cache).await,
            RouteResult::Execute(_, _, None)
        ));
        assert!(matches!(
            &*rc.resolve_request("/assets/app.js", &cache).await,
            RouteResult::Serve(_)
        ));
        for uri in ["/lib/secret.php", "/notes.txt", "/anything"] {
            let result = rc.resolve_request(uri, &cache).await;
            assert!(is_denied(&result, 404), "{uri}: {result:?}");
        }
    }

    #[tokio::test]
    async fn trailing_slash_does_not_bypass_an_allowlist() {
        // `!*/` re-includes every directory, the gitignore way to allow files
        // below them. A trailing slash on a file must not count as one.
        let (_dir, cache, rc) = setup(
            &[
                ("secret.txt", b"s"),
                ("secret.php", b"<?php"),
                ("app.js", b"js"),
            ],
            Some("*\n!*/\n!*.js\n"),
            &[],
            None,
            false,
        );
        for uri in [
            "/secret.txt",
            "/secret.txt/",
            "/secret.txt%2F",
            "/secret.php/",
        ] {
            let result = rc.resolve_request(uri, &cache).await;
            assert!(is_denied(&result, 404), "{uri}: {result:?}");
        }
        assert!(matches!(
            &*rc.resolve_request("/app.js", &cache).await,
            RouteResult::Serve(_)
        ));
    }

    #[tokio::test]
    async fn path_info_does_not_bypass_an_allowlist() {
        // Traditional mode runs `secret.php` for `/secret.php/x.js`. Judged
        // as a directory (`!*/`) and by its full path (`!*.js`), it would
        // run a script the allowlist denies.
        let rules = Some("*\n!*/\n!*.js\n");
        let layout: &[(&str, &[u8])] = &[
            ("secret.php", b"<?php"),
            ("index.php", b"<?php"),
            ("app.js", b"js"),
        ];
        let (_dir, cache, rc) = setup(layout, rules, &[], None, false);
        for uri in ["/secret.php/x.js", "/secret.PHP/x.js", "/a/secret.php/x.js"] {
            let result = rc.resolve_request(uri, &cache).await;
            assert!(is_denied(&result, 404), "{uri}: {result:?}");
        }
        // Framework mode runs no script named in the URI: the front
        // controller gets the request, as the path's own judgement says.
        let (_dir, cache, rc) = setup(layout, rules, &[], Some("index.php"), false);
        let result = rc.resolve_request("/secret.php/x.js", &cache).await;
        assert!(
            matches!(&*result, RouteResult::Execute(p, None, None) if p.ends_with("index.php")),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn deep_paths_are_refused_before_matching() {
        // Traditional mode sends a missing extensionless path to the root
        // index, so a path under the cap runs it and one past the cap — 65
        // segments — is told apart by its NotFound.
        let (_dir, cache, rc) = setup(
            &[("index.php", b"<?php")],
            Some("vendor/\n"),
            &[],
            None,
            false,
        );
        let at_cap = "/a".repeat(64);
        let over_cap = format!("{at_cap}/a");
        assert!(matches!(
            &*rc.resolve_request(&at_cap, &cache).await,
            RouteResult::Execute(_, _, None)
        ));
        assert!(matches!(
            &*rc.resolve_request(&over_cap, &cache).await,
            RouteResult::NotFound
        ));
        assert!(!cached(&rc, &over_cap), "a refusal was cached");
        let deep_vendor = format!("/vendor{}", "/a".repeat(10_000));
        assert!(matches!(
            &*rc.resolve_request(&deep_vendor, &cache).await,
            RouteResult::NotFound
        ));
    }

    #[tokio::test]
    async fn long_deep_paths_are_refused_before_matching() {
        // Under the segment cap, but each parent re-check scans ~1 KB more.
        let (_dir, cache, rc) = setup(
            &[("index.php", b"<?php")],
            Some("vendor/\n"),
            &[],
            None,
            false,
        );
        let uri = format!("/{}", vec!["a".repeat(1000); 63].join("/"));
        assert!(matches!(
            &*rc.resolve_request(&uri, &cache).await,
            RouteResult::NotFound
        ));
        assert!(!cached(&rc, &uri), "a refusal was cached");
    }

    #[tokio::test]
    async fn deep_paths_route_as_before_without_a_deny_file() {
        let (_dir, cache, rc) = setup(&[("index.php", b"<?php")], None, &[], None, false);
        assert!(matches!(
            &*rc.resolve_request(&"/a".repeat(65), &cache).await,
            RouteResult::Execute(_, _, None)
        ));
    }

    #[tokio::test]
    async fn deep_paths_route_as_before_when_no_rule_can_exclude() {
        // Nothing to protect: the depth cap would only turn a route into 404.
        for rules in ["", "# only a comment\n", "!keep\n"] {
            let (_dir, cache, rc) =
                setup(&[("index.php", b"<?php")], Some(rules), &[], None, false);
            assert!(
                matches!(
                    &*rc.resolve_request(&"/a".repeat(65), &cache).await,
                    RouteResult::Execute(_, _, None)
                ),
                "{rules:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_script_named_in_another_case_is_denied() {
        // Routing runs `.PHP` as PHP, so `*.php` must cover it on a
        // case-sensitive filesystem too.
        let (_dir, cache, rc) = setup(
            &[("index.php", b"<?php"), ("uploads/shell.PHP", b"<?php")],
            Some("/uploads/*.php\n"),
            &[],
            None,
            false,
        );
        let result = rc.resolve_request("/uploads/shell.PHP", &cache).await;
        assert!(is_denied(&result, 404), "{result:?}");
    }

    #[tokio::test]
    async fn the_documented_allowlist_serves_acme_challenges() {
        // `/.well-known/` passes the dot-segment screen and reaches the
        // rules, so an allowlist must re-include it or HTTP-01 renewal fails.
        let rules = "*\n!/index.php\n!/assets/\n!/assets/**\n!/.well-known/\n!/.well-known/**\n";
        let layout: &[(&str, &[u8])] = &[
            ("index.php", b"<?php"),
            (".well-known/acme-challenge/tok", b"tok"),
            ("secret.txt", b"s"),
        ];
        let (_dir, cache, rc) = setup(layout, Some(rules), &[], None, false);
        let result = rc
            .resolve_request("/.well-known/acme-challenge/tok", &cache)
            .await;
        assert!(matches!(&*result, RouteResult::Serve(_)), "{result:?}");
        let result = rc.resolve_request("/secret.txt", &cache).await;
        assert!(is_denied(&result, 404), "{result:?}");
    }
}
