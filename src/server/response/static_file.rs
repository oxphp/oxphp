use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use futures_util::StreamExt;
use http::{header, Response, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use tokio_util::io::ReaderStream;

use crate::types::{full_body, ResponseBody};

/// Maximum individual file size eligible for content caching (1 MiB).
const MAX_CACHE_FILE_SIZE: usize = 1_048_576;

/// Maximum total bytes held in the content cache (64 MiB).
const MAX_CACHE_TOTAL_BYTES: usize = 67_108_864;

/// Cached filesystem entry type.
#[derive(Debug, Clone, Copy)]
pub enum FileType {
    File,
    Dir,
}

/// Metadata cache entry with LRU timestamp.
struct MetaEntry {
    file_type: Option<FileType>,
    last_used: u64,
}

/// Content cache entry with LRU timestamp.
struct ContentEntry {
    bytes: Bytes,
    mime_type: Arc<str>,
    last_used: u64,
}

/// Canonical path cache entry with LRU timestamp.
struct CanonEntry {
    path: Option<PathBuf>,
    last_used: u64,
}

struct FileCacheInner {
    /// Metadata cache: path → entry
    meta: HashMap<String, MetaEntry>,
    capacity: usize,

    /// Content cache: path → entry
    content: HashMap<String, ContentEntry>,
    content_total_bytes: usize,

    /// Canonical path cache: path → entry
    canonical: HashMap<String, CanonEntry>,

    /// Monotonic counter incremented on every cache access.
    counter: u64,
}

/// LRU file cache to reduce filesystem syscalls during routing,
/// with an optional content cache for small files.
/// Uses `Mutex<HashMap>` with counter-based LRU eviction.
pub struct FileCache {
    inner: Mutex<FileCacheInner>,
}

impl FileCache {
    /// Create a new file cache with the given metadata entry capacity.
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(FileCacheInner {
                meta: HashMap::with_capacity(capacity),
                capacity,
                content: HashMap::new(),
                content_total_bytes: 0,
                canonical: HashMap::with_capacity(capacity),
                counter: 0,
            }),
        }
    }

    /// Check the cache for a path. Returns (file_type, was_cached).
    pub async fn check(&self, path: &str) -> (Option<FileType>, bool) {
        // Check cache (short lock, no await inside)
        {
            let mut guard = self.inner.lock().unwrap();
            let inner = &mut *guard; // reborrow for field-level splitting
            if let Some(entry) = inner.meta.get_mut(path) {
                entry.last_used = inner.counter;
                inner.counter += 1;
                return (entry.file_type, true);
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
            if inner.meta.len() >= inner.capacity {
                if let Some(lru_key) = inner
                    .meta
                    .iter()
                    .min_by_key(|(_, e)| e.last_used)
                    .map(|(k, _)| k.clone())
                {
                    inner.meta.remove(&lru_key);
                }
            }

            let ts = inner.counter;
            inner.counter += 1;
            inner.meta.insert(
                path.to_string(),
                MetaEntry {
                    file_type,
                    last_used: ts,
                },
            );
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

    /// Get cached file content and MIME type. Returns `None` on cache miss.
    /// O(1) clone via `Bytes` Arc increment + `Arc<str>` bump.
    pub fn get_content(&self, key: &str) -> Option<(Bytes, Arc<str>)> {
        let mut guard = self.inner.lock().unwrap();
        let inner = &mut *guard;
        if let Some(entry) = inner.content.get_mut(key) {
            entry.last_used = inner.counter;
            inner.counter += 1;
            Some((entry.bytes.clone(), entry.mime_type.clone()))
        } else {
            None
        }
    }

    /// Insert file content into the cache. Skips files larger than MAX_CACHE_FILE_SIZE.
    /// Evicts LRU entries when total cache size exceeds MAX_CACHE_TOTAL_BYTES.
    pub fn insert_content(&self, key: String, bytes: Bytes, mime_type: Arc<str>) {
        if bytes.len() > MAX_CACHE_FILE_SIZE {
            return;
        }

        let mut inner = self.inner.lock().unwrap();

        // Evict LRU entries while over budget
        while inner.content_total_bytes + bytes.len() > MAX_CACHE_TOTAL_BYTES {
            if let Some(lru_key) = inner
                .content
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone())
            {
                if let Some(evicted) = inner.content.remove(&lru_key) {
                    inner.content_total_bytes -= evicted.bytes.len();
                }
            } else {
                break;
            }
        }

        // Remove old entry if re-inserting same key
        if let Some(old) = inner.content.remove(&key) {
            inner.content_total_bytes -= old.bytes.len();
        }

        let ts = inner.counter;
        inner.counter += 1;
        inner.content_total_bytes += bytes.len();
        inner.content.insert(
            key,
            ContentEntry {
                bytes,
                mime_type,
                last_used: ts,
            },
        );
    }

    /// Get a cached canonical path. Returns `None` on cache miss.
    /// The inner `Option<PathBuf>` distinguishes: `Some(path)` = canonicalization
    /// succeeded, `None` = file did not exist at cache time.
    pub fn get_canonical(&self, key: &str) -> Option<Option<PathBuf>> {
        let mut guard = self.inner.lock().unwrap();
        let inner = &mut *guard;
        if let Some(entry) = inner.canonical.get_mut(key) {
            entry.last_used = inner.counter;
            inner.counter += 1;
            Some(entry.path.clone())
        } else {
            None
        }
    }

    /// Cache a canonical path result. Uses the same capacity as the metadata cache.
    pub fn insert_canonical(&self, key: String, canonical: Option<PathBuf>) {
        let mut inner = self.inner.lock().unwrap();

        // Evict LRU entry if at capacity
        if inner.canonical.len() >= inner.capacity {
            if let Some(lru_key) = inner
                .canonical
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, _)| k.clone())
            {
                inner.canonical.remove(&lru_key);
            }
        }

        // Remove old entry if re-inserting same key
        inner.canonical.remove(&key);

        let ts = inner.counter;
        inner.counter += 1;
        inner.canonical.insert(
            key,
            CanonEntry {
                path: canonical,
                last_used: ts,
            },
        );
    }
}

/// Re-canonicalize a file path and verify it stays within the document root.
/// Returns `false` if the path escapes the root (TOCTOU mitigation).
fn verify_canonical(file_path: &Path, canonical_root: &Path) -> bool {
    match std::fs::canonicalize(file_path) {
        Ok(real) => real.starts_with(canonical_root),
        Err(_) => false,
    }
}

/// Serve a static file with MIME type detection, content caching, and streaming.
///
/// Files ≤ 1 MiB are cached in memory. Files > 1 MiB are streamed from disk.
/// If `canonical_root` is provided, re-validates the file path at serve time
/// to mitigate TOCTOU symlink swap attacks.
pub async fn serve(
    file_path: &Path,
    cache: &FileCache,
    canonical_root: Option<&Path>,
) -> Result<Response<ResponseBody>, crate::types::BoxError> {
    let cache_key = file_path.to_string_lossy();

    // 1. Check content cache (already validated at insertion time)
    if let Some((cached_bytes, cached_mime)) = cache.get_content(&cache_key) {
        let len = cached_bytes.len();
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, &*cached_mime)
            .header(header::CONTENT_LENGTH, len)
            .body(full_body(cached_bytes))?);
    }

    // Cache miss — compute MIME type
    let mime_type: Arc<str> = mime_guess::from_path(file_path)
        .first_or_octet_stream()
        .to_string()
        .into();

    // 2. TOCTOU mitigation: re-canonicalize before reading from disk
    if let Some(root) = canonical_root {
        if !verify_canonical(file_path, root) {
            tracing::warn!(
                path = %file_path.display(),
                "TOCTOU: path escaped document root at serve time"
            );
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(full_body(Bytes::from_static(b"404 Not Found")))?);
        }
    }

    // 3. Get file metadata for size
    let metadata = match tokio::fs::metadata(file_path).await {
        Ok(m) => m,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(full_body(Bytes::from_static(b"404 Not Found")))?);
        }
        Err(e) => return Err(e.into()),
    };

    let file_size = metadata.len() as usize;

    // 4. Small file: read fully, cache, return buffered body
    if file_size <= MAX_CACHE_FILE_SIZE {
        let contents = match tokio::fs::read(file_path).await {
            Ok(c) => c,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Ok(Response::builder()
                    .status(StatusCode::NOT_FOUND)
                    .body(full_body(Bytes::from_static(b"404 Not Found")))?);
            }
            Err(e) => return Err(e.into()),
        };

        let bytes = Bytes::from(contents);
        cache.insert_content(cache_key.into_owned(), bytes.clone(), mime_type.clone());

        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header(header::CONTENT_TYPE, &*mime_type)
            .header(header::CONTENT_LENGTH, bytes.len())
            .body(full_body(bytes))?);
    }

    // 5. Large file: stream from disk
    let file = match tokio::fs::File::open(file_path).await {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(full_body(Bytes::from_static(b"404 Not Found")))?);
        }
        Err(e) => return Err(e.into()),
    };

    let stream = ReaderStream::new(file);
    let stream_body =
        StreamBody::new(stream.map(|result: Result<Bytes, io::Error>| result.map(Frame::data)));

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, &*mime_type)
        .header(header::CONTENT_LENGTH, file_size)
        .body(BodyExt::boxed(stream_body))?)
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
        assert!(inner.meta.len() <= 2);
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
        assert!(inner.meta.contains_key(&f1.to_string_lossy().to_string()));
        assert!(!inner.meta.contains_key(&f2.to_string_lossy().to_string()));
        assert!(inner.meta.contains_key(&f3.to_string_lossy().to_string()));
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

        let cache = FileCache::new(10);
        let response = serve(&file_path, &cache, None).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let ct = response.headers().get(header::CONTENT_TYPE).unwrap();
        assert!(ct.to_str().unwrap().contains("text/html"));
    }

    #[tokio::test]
    async fn test_serve_css_content_type() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("style.css");
        fs::write(&file_path, "body {}").unwrap();

        let cache = FileCache::new(10);
        let response = serve(&file_path, &cache, None).await.unwrap();
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

        let cache = FileCache::new(10);
        let response = serve(&file_path, &cache, None).await.unwrap();
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
        let cache = FileCache::new(10);
        let response = serve(Path::new("/nonexistent/file.txt"), &cache, None)
            .await
            .unwrap();
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

    // --- Content cache tests ---

    #[test]
    fn test_content_cache_hit_miss() {
        let cache = FileCache::new(10);

        // Miss
        assert!(cache.get_content("/foo.txt").is_none());

        // Insert and hit
        cache.insert_content(
            "/foo.txt".to_string(),
            Bytes::from_static(b"hello"),
            "text/plain".into(),
        );
        let hit = cache.get_content("/foo.txt");
        assert!(hit.is_some());
        let (bytes, mime) = hit.unwrap();
        assert_eq!(bytes, &b"hello"[..]);
        assert_eq!(&*mime, "text/plain");
    }

    #[test]
    fn test_content_cache_skips_large_files() {
        let cache = FileCache::new(10);

        // File larger than MAX_CACHE_FILE_SIZE should be skipped
        let large = Bytes::from(vec![0u8; MAX_CACHE_FILE_SIZE + 1]);
        cache.insert_content(
            "big.bin".to_string(),
            large,
            "application/octet-stream".into(),
        );
        assert!(cache.get_content("big.bin").is_none());
    }

    #[test]
    fn test_content_cache_eviction() {
        let cache = FileCache::new(10);

        // Insert two entries that together exceed MAX_CACHE_TOTAL_BYTES
        // Use MAX_CACHE_FILE_SIZE entries to fill faster
        let data = Bytes::from(vec![0u8; MAX_CACHE_FILE_SIZE]);
        let mime: Arc<str> = "application/octet-stream".into();

        let entries_to_fill = MAX_CACHE_TOTAL_BYTES / MAX_CACHE_FILE_SIZE;
        for i in 0..entries_to_fill {
            cache.insert_content(format!("file_{}", i), data.clone(), mime.clone());
        }

        // All entries should be present
        for i in 0..entries_to_fill {
            assert!(
                cache.get_content(&format!("file_{}", i)).is_some(),
                "file_{} should be cached",
                i
            );
        }

        // One more should trigger eviction of the LRU entry
        cache.insert_content(
            "overflow".to_string(),
            data,
            "application/octet-stream".into(),
        );

        // First entry should be evicted
        assert!(cache.get_content("file_0").is_none());
        assert!(cache.get_content("overflow").is_some());
    }

    #[test]
    fn test_canonical_cache_hit_miss() {
        let cache = FileCache::new(10);

        // Miss
        assert!(cache.get_canonical("/some/path").is_none());

        // Insert a successful canonicalization
        cache.insert_canonical(
            "/some/path".to_string(),
            Some(PathBuf::from("/real/canonical/path")),
        );
        let hit = cache.get_canonical("/some/path");
        assert!(hit.is_some());
        assert_eq!(hit.unwrap(), Some(PathBuf::from("/real/canonical/path")));

        // Insert a failed canonicalization (file not found)
        cache.insert_canonical("/missing/file".to_string(), None);
        let hit = cache.get_canonical("/missing/file");
        assert_eq!(hit, Some(None));
    }

    #[test]
    fn test_canonical_cache_eviction() {
        let cache = FileCache::new(2);

        cache.insert_canonical("a".to_string(), Some(PathBuf::from("/a")));
        cache.insert_canonical("b".to_string(), Some(PathBuf::from("/b")));

        // At capacity, inserting c should evict a (LRU)
        cache.insert_canonical("c".to_string(), Some(PathBuf::from("/c")));

        assert!(cache.get_canonical("a").is_none());
        assert!(cache.get_canonical("b").is_some());
        assert!(cache.get_canonical("c").is_some());
    }

    #[tokio::test]
    async fn test_serve_caches_small_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("small.txt");
        fs::write(&file_path, "cached content").unwrap();

        let cache = FileCache::new(10);
        let cache_key = file_path.to_string_lossy().to_string();

        // Before serve: no cache entry
        assert!(cache.get_content(&cache_key).is_none());

        // Serve populates cache
        let _response = serve(&file_path, &cache, None).await.unwrap();
        let cached = cache.get_content(&cache_key);
        assert!(cached.is_some());
        let (bytes, mime) = cached.unwrap();
        assert_eq!(bytes, &b"cached content"[..]);
        assert!(mime.contains("text/plain"));

        // Second serve should hit cache (we can't directly assert cache hit,
        // but we verify it still works)
        let response2 = serve(&file_path, &cache, None).await.unwrap();
        assert_eq!(response2.status(), StatusCode::OK);
    }
}
