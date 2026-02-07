use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::Path;
use std::sync::Mutex;

use http::{header, Response, StatusCode};
use http_body_util::Full;
use hyper::body::Bytes;

/// Cached filesystem entry type.
#[derive(Debug, Clone, Copy)]
pub enum FileType {
    File,
    Dir,
}

struct FileCacheInner {
    map: HashMap<String, Option<FileType>>,
    order: VecDeque<String>,
    capacity: usize,
}

/// LRU file cache to reduce filesystem syscalls during routing.
/// Uses `Mutex<HashMap>` with LRU eviction via `VecDeque`.
pub struct FileCache {
    inner: Mutex<FileCacheInner>,
}

impl FileCache {
    /// Create a new file cache with the given capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(FileCacheInner {
                map: HashMap::with_capacity(capacity),
                order: VecDeque::with_capacity(capacity),
                capacity,
            }),
        }
    }

    /// Check the cache for a path. Returns (file_type, was_cached).
    pub async fn check(&self, path: &str) -> (Option<FileType>, bool) {
        // Check cache (short lock, no await inside)
        {
            let mut inner = self.inner.lock().unwrap();
            if let Some(&file_type) = inner.map.get(path) {
                // LRU: move to back of order queue
                if let Some(pos) = inner.order.iter().position(|k| k == path) {
                    let key = inner.order.remove(pos).unwrap();
                    inner.order.push_back(key);
                }
                return (file_type, true);
            }
        }

        // Cache miss — async filesystem check
        let file_type = match tokio::fs::metadata(path).await {
            Ok(meta) if meta.is_file() => Some(FileType::File),
            Ok(meta) if meta.is_dir() => Some(FileType::Dir),
            _ => None,
        };

        // Insert into cache with LRU eviction (short lock)
        {
            let mut inner = self.inner.lock().unwrap();

            // Evict LRU entry if at capacity
            if inner.map.len() >= inner.capacity {
                if let Some(oldest) = inner.order.pop_front() {
                    inner.map.remove(&oldest);
                }
            }

            let key = path.to_string();
            inner.map.insert(key.clone(), file_type);
            inner.order.push_back(key);
        }

        (file_type, false)
    }

    /// Returns true if path is a regular file.
    pub async fn is_file(&self, path: &str) -> bool {
        matches!(self.check(path).await.0, Some(FileType::File))
    }

    /// Returns true if path is a directory.
    #[allow(dead_code)]
    pub async fn is_dir(&self, path: &str) -> bool {
        matches!(self.check(path).await.0, Some(FileType::Dir))
    }
}

/// Serve a static file with MIME type detection.
pub async fn serve(file_path: &Path) -> Result<Response<Full<Bytes>>, crate::types::BoxError> {
    let contents = match tokio::fs::read(file_path).await {
        Ok(c) => c,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Full::new(Bytes::from_static(b"404 Not Found")))?);
        }
        Err(e) => return Err(e.into()),
    };

    let mime_type = mime_guess::from_path(file_path)
        .first_or_octet_stream()
        .to_string();

    let response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &mime_type)
        .header(header::CONTENT_LENGTH, contents.len())
        .body(Full::new(Bytes::from(contents)))?;

    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use tempfile::TempDir;

    #[tokio::test]
    async fn test_file_cache_hit_miss() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("test.txt");
        fs::write(&file_path, "hello").unwrap();

        let cache = FileCache::new(10);

        // First call: cache miss
        let (ft, cached) = cache.check(&file_path.to_string_lossy()).await;
        assert!(matches!(ft, Some(FileType::File)));
        assert!(!cached);

        // Second call: cache hit
        let (ft, cached) = cache.check(&file_path.to_string_lossy()).await;
        assert!(matches!(ft, Some(FileType::File)));
        assert!(cached);
    }

    #[tokio::test]
    async fn test_file_cache_nonexistent() {
        let cache = FileCache::new(10);
        let (ft, _) = cache.check("/nonexistent/path/file.txt").await;
        assert!(ft.is_none());
    }

    #[tokio::test]
    async fn test_file_cache_capacity() {
        let dir = TempDir::new().unwrap();
        let cache = FileCache::new(2);

        // Fill cache to capacity
        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        let f3 = dir.path().join("c.txt");
        fs::write(&f1, "a").unwrap();
        fs::write(&f2, "b").unwrap();
        fs::write(&f3, "c").unwrap();

        cache.check(&f1.to_string_lossy()).await;
        cache.check(&f2.to_string_lossy()).await;

        // Cache is full at 2, adding third should evict one
        cache.check(&f3.to_string_lossy()).await;

        let inner = cache.inner.lock().unwrap();
        assert!(inner.map.len() <= 2);
    }

    #[tokio::test]
    async fn test_file_cache_lru_eviction() {
        let dir = TempDir::new().unwrap();
        let cache = FileCache::new(2);

        let f1 = dir.path().join("a.txt");
        let f2 = dir.path().join("b.txt");
        let f3 = dir.path().join("c.txt");
        fs::write(&f1, "a").unwrap();
        fs::write(&f2, "b").unwrap();
        fs::write(&f3, "c").unwrap();

        // Insert f1, f2
        cache.check(&f1.to_string_lossy()).await;
        cache.check(&f2.to_string_lossy()).await;

        // Access f1 again — makes f2 the LRU entry
        cache.check(&f1.to_string_lossy()).await;

        // Insert f3 — should evict f2 (LRU), not f1
        cache.check(&f3.to_string_lossy()).await;

        let inner = cache.inner.lock().unwrap();
        assert!(inner.map.contains_key(&f1.to_string_lossy().to_string()));
        assert!(!inner.map.contains_key(&f2.to_string_lossy().to_string()));
        assert!(inner.map.contains_key(&f3.to_string_lossy().to_string()));
    }

    #[tokio::test]
    async fn test_file_cache_is_dir() {
        let dir = TempDir::new().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();

        let cache = FileCache::new(10);
        assert!(cache.is_dir(&sub.to_string_lossy()).await);
        assert!(!cache.is_file(&sub.to_string_lossy()).await);
    }

    #[tokio::test]
    async fn test_serve_html_content_type() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("page.html");
        fs::write(&file_path, "<html>Hello</html>").unwrap();

        let response = serve(&file_path).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ct = response.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(ct.to_str().unwrap().contains("text/html"));
    }

    #[tokio::test]
    async fn test_serve_css_content_type() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("style.css");
        fs::write(&file_path, "body {}").unwrap();

        let response = serve(&file_path).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ct = response.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(ct.to_str().unwrap().contains("text/css"));
    }

    #[tokio::test]
    async fn test_serve_content_length() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        let content = "Hello, World!";
        fs::write(&file_path, content).unwrap();

        let response = serve(&file_path).await.unwrap();
        let cl = response
            .headers()
            .get(header::CONTENT_LENGTH)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cl, content.len().to_string());
    }

    #[tokio::test]
    async fn test_serve_nonexistent_returns_404() {
        let response = serve(Path::new("/nonexistent/file.txt")).await.unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_is_file_and_is_dir() {
        let dir = TempDir::new().unwrap();
        let file = dir.path().join("test.txt");
        fs::write(&file, "x").unwrap();

        let cache = Arc::new(FileCache::new(10));
        assert!(cache.is_file(&file.to_string_lossy()).await);
        assert!(!cache.is_dir(&file.to_string_lossy()).await);
    }
}
