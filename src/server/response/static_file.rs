use std::io;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use bytes::Bytes;
use futures_util::StreamExt;
use http::{header, HeaderMap, Response, StatusCode};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use lru::LruCache;
use parking_lot::{Mutex, RwLock};
use tokio_util::io::ReaderStream;

use crate::server::compression::{Coding, Levels};
use crate::types::{full_body, ResponseBody};

/// Maximum individual file size eligible for content caching (1 MiB).
const MAX_CACHE_FILE_SIZE: usize = 1_048_576;

/// Maximum total bytes held in the content cache (64 MiB).
const MAX_CACHE_TOTAL_BYTES: usize = 67_108_864;

/// How many files above the cache limit may be held in memory at once for the
/// sake of compressing them. Such a file is read whole, and it and its
/// compressed output stay resident until the response has been written, so the
/// only other ceiling on this path is `MAX_CONNECTIONS` — ten thousand by
/// default, and reachable by anyone who can ask for a bundle twice. Sixteen
/// puts the worst case in the same order as the content cache's own budget.
const MAX_OFF_DISK_COMPRESSIONS: usize = 16;

/// Carried on the response so the permit outlives the bytes it accounts for —
/// dropped when the response is, which is after the body reaches the wire.
#[derive(Clone)]
struct OffDiskCompression(#[allow(dead_code)] Arc<tokio::sync::OwnedSemaphorePermit>);

/// Revalidation window when `STATIC_REVALIDATE=on`. A cached entry's mtime is
/// re-checked via `stat()` at most once per window, not on every hit — so the
/// syscall cost is amortized to ~1 stat / file / window instead of one (or two)
/// stats per request. Changes on disk become visible within this bound.
const STATIC_REVALIDATE_TTL: Duration = Duration::from_secs(3);

/// Cached filesystem entry type.
#[derive(Debug, Clone, Copy)]
pub enum FileType {
    File,
    Dir,
}

/// Content cache entry (LRU ordering managed by LruCache).
struct ContentEntry {
    bytes: Bytes,
    mime_type: Arc<str>,
    modified: SystemTime,
    etag: Arc<str>,
    /// Pre-formatted HTTP date for Last-Modified header (avoids per-request formatting).
    last_modified_str: Arc<str>,
    /// When this entry's mtime was last verified against disk. Used by TTL-based
    /// revalidation to skip the `stat()` while the window is still open.
    last_checked: Instant,
    /// One encoded representation of `bytes` per content coding, each built
    /// once in the background and served to clients that negotiated it. Kept
    /// inside the entry rather than in a cache of its own so they share the
    /// identity bytes' validator: when revalidation evicts the entry because
    /// the file changed on disk, no artifact can outlive the bytes it was made
    /// from. Indexed by [`Coding::slot`].
    artifacts: [ArtifactState; Coding::COUNT],
}

/// Where an entry stands with respect to one coding's artifact.
#[derive(Clone)]
pub enum ArtifactState {
    /// Not built yet — a hit on this entry asks for one.
    Absent,
    /// Built, and smaller than the identity bytes.
    Ready(Bytes),
    /// Compression did not make these bytes smaller. Recorded so the entry
    /// stops asking: without it every hit on an incompressible file would
    /// spawn another top-of-range compression that throws its result away.
    Rejected,
}

impl ContentEntry {
    /// Bytes this entry charges against the cache budget — identity plus every
    /// artifact hanging off it.
    fn footprint(&self) -> usize {
        self.bytes.len()
            + self
                .artifacts
                .iter()
                .map(|artifact| match artifact {
                    ArtifactState::Ready(bytes) => bytes.len(),
                    ArtifactState::Absent | ArtifactState::Rejected => 0,
                })
                .sum::<usize>()
    }
}

/// Outcome of a content-cache lookup that already factored in conditional
/// (304) headers and TTL revalidation, computed under a single cache access.
pub enum Lookup {
    /// Cached and the request's conditional headers indicate 304 Not Modified.
    NotModified {
        etag: Arc<str>,
        last_modified_str: Arc<str>,
    },
    /// Cached content to serve as a 200 (or range) response.
    Content {
        bytes: Bytes,
        mime_type: Arc<str>,
        modified: SystemTime,
        etag: Arc<str>,
        last_modified_str: Arc<str>,
        /// Where this entry stands with respect to the artifact for the
        /// coding this request negotiated. `Absent` when it negotiated none —
        /// a client that accepts no coding has no artifact to consider, and
        /// the caller does not look.
        artifact: ArtifactState,
    },
}

/// Reports that this response body is a cached artifact rather than the
/// identity bytes, and how many bytes that saved. Static serving cannot reach
/// the metrics registry, and the compression layer downstream sees an
/// already-encoded response and has nothing to measure — without this the
/// bytes-saved counter would quietly stop counting static traffic.
#[derive(Clone, Copy)]
pub struct PrecompressedSaving(pub usize);

/// Asks the caller to build one coding's artifact for a cached entry that does
/// not have it yet. Static serving decides (it holds the cache and the
/// request), the caller acts (it holds the runtime handle and an owned cache
/// reference) — so the decision stays where the state is and the side effect
/// stays where the machinery is.
#[derive(Clone)]
pub struct ArtifactWanted {
    pub key: String,
    pub coding: Coding,
    pub bytes: Bytes,
    pub modified: SystemTime,
}

/// Content cache with its own byte budget. Kept as a single struct so the
/// LRU and the running total stay consistent under one lock.
struct ContentCache {
    entries: LruCache<String, ContentEntry>,
    total_bytes: usize,
}

/// LRU file cache to reduce filesystem syscalls during routing,
/// with an optional content cache for small files.
///
/// Three independent locks rather than one big `RwLock<Inner>`:
/// - `meta`/`canonical` are updated on every routing miss and held very
///   briefly; `parking_lot::Mutex` has the lowest uncontended cost.
/// - `content` is read-heavy after warmup and genuinely benefits from
///   reader parallelism under static-file load; `parking_lot::RwLock`
///   pays a lower read-lock tax than `std::sync::RwLock` (pthread) and
///   does not starve readers under occasional writes.
///
/// Splitting them removes false serialization between routing (which
/// touches `meta`/`canonical`) and static serving (which touches
/// `content`).
pub struct FileCache {
    meta: Mutex<LruCache<String, Option<FileType>>>,
    content: RwLock<ContentCache>,
    canonical: Mutex<LruCache<String, Option<PathBuf>>>,
    /// When `Some(ttl)`, content lookups re-check the file mtime via `stat()` at
    /// most once per `ttl` window; entries whose mtime changed are evicted.
    /// `None` disables revalidation (cached bytes served until LRU eviction).
    revalidate_ttl: Option<Duration>,
    /// Artifacts being built right now, keyed by cache key and coding. Its own lock:
    /// claiming must not queue behind the content lock, which static serving
    /// holds on every hit.
    artifacts_in_flight: Mutex<std::collections::HashSet<(String, Coding)>>,
    /// Permits for reading a file too large to cache whole so it can be
    /// compressed. Deliberately never awaited: a request that finds none free
    /// streams the file uncompressed — what it would have been served before
    /// this path existed — rather than queueing behind the memory it wanted.
    off_disk_compression: Arc<tokio::sync::Semaphore>,
}

/// Holds a single-flight claim on one key's artifact for as long as the job
/// runs. Releasing on drop covers the paths that do not reach the insert —
/// incompressible input, a panic inside the blocking task, a shutdown that
/// drops it — any of which would otherwise wedge that key permanently.
pub struct ArtifactClaim {
    cache: Arc<FileCache>,
    key: String,
    coding: Coding,
}

impl ArtifactClaim {
    pub fn key(&self) -> &str {
        &self.key
    }
}

impl Drop for ArtifactClaim {
    fn drop(&mut self) {
        // The set is keyed by a tuple, which has no borrowed form to look up
        // by — so the key is moved into a throwaway one rather than cloned.
        self.cache
            .artifacts_in_flight
            .lock()
            .remove(&(std::mem::take(&mut self.key), self.coding));
    }
}

impl FileCache {
    /// Create a new file cache with the given metadata entry capacity.
    pub fn new(capacity: usize) -> Self {
        Self::with_revalidation(capacity, false)
    }

    /// Create a file cache with content revalidation toggled by `validate`.
    /// When `validate` is true, content lookups re-check the file mtime against
    /// disk at most once per [`STATIC_REVALIDATE_TTL`] window and evict entries
    /// whose mtime has changed.
    pub fn with_revalidation(capacity: usize, validate: bool) -> Self {
        Self::with_revalidation_ttl(capacity, validate.then_some(STATIC_REVALIDATE_TTL))
    }

    /// Create a file cache with an explicit revalidation window. `None` disables
    /// revalidation; `Some(Duration::ZERO)` revalidates on every hit.
    pub fn with_revalidation_ttl(capacity: usize, revalidate_ttl: Option<Duration>) -> Self {
        let cap = NonZeroUsize::new(capacity).unwrap_or(NonZeroUsize::new(1).unwrap());
        Self {
            meta: Mutex::new(LruCache::new(cap)),
            content: RwLock::new(ContentCache {
                entries: LruCache::unbounded(),
                total_bytes: 0,
            }),
            canonical: Mutex::new(LruCache::new(cap)),
            revalidate_ttl,
            artifacts_in_flight: Mutex::new(std::collections::HashSet::new()),
            off_disk_compression: Arc::new(tokio::sync::Semaphore::new(MAX_OFF_DISK_COMPRESSIONS)),
        }
    }

    /// Build a [`Lookup`] from a live cache entry, applying the conditional
    /// (304) check when `try_304` is set.
    fn build_lookup(
        entry: &ContentEntry,
        headers: &HeaderMap,
        try_304: bool,
        coding: Option<Coding>,
    ) -> Lookup {
        if try_304 && check_not_modified(headers, &entry.etag, &entry.modified) {
            Lookup::NotModified {
                etag: entry.etag.clone(),
                last_modified_str: entry.last_modified_str.clone(),
            }
        } else {
            Lookup::Content {
                bytes: entry.bytes.clone(),
                mime_type: entry.mime_type.clone(),
                modified: entry.modified,
                etag: entry.etag.clone(),
                last_modified_str: entry.last_modified_str.clone(),
                artifact: coding.map_or(ArtifactState::Absent, |coding| {
                    entry.artifacts[coding.slot()].clone()
                }),
            }
        }
    }

    /// Combined 304 + content lookup in a single cache access. Resolves the
    /// conditional check and content fetch together so a served request stats
    /// the file at most once (and only when the revalidation window elapsed).
    /// `try_304` should be set only when a 304 is actually serviceable
    /// (conditional method + a Cache-Control to echo). `coding` is the coding
    /// this request negotiated *for stored copies* — the one whose artifact is
    /// reported back in [`Lookup::Content`], which is not necessarily the one
    /// the request path would compress with.
    pub fn lookup(
        &self,
        key: &str,
        headers: &HeaderMap,
        try_304: bool,
        coding: Option<Coding>,
    ) -> Option<Lookup> {
        // Fast path: read lock. The entry is served directly when revalidation
        // is off, or the window has not elapsed — no syscall, full reader
        // parallelism. LRU order is not promoted here (would need a write lock);
        // promotion happens on the slow path below, once per window.
        {
            let guard = self.content.read();
            let entry = guard.entries.peek(key)?;
            let stale =
                matches!(self.revalidate_ttl, Some(ttl) if entry.last_checked.elapsed() >= ttl);
            if !stale {
                return Some(Self::build_lookup(entry, headers, try_304, coding));
            }
        }
        // Slow path: window elapsed. Claim the revalidation under the write
        // lock, then stat() outside it. A single `get_mut` re-checks the window,
        // promotes the entry to MRU (restoring LRU ordering for hot files), and
        // — if we win the claim — bumps `last_checked` before the lock is
        // released. Concurrent callers that arrive while we stat then see a
        // fresh window and skip their own syscall, so a thundering herd on one
        // file collapses to a single stat() per window.
        //
        // The stat runs with no lock held, so it never blocks readers on the
        // content lock. It is a synchronous `std::fs` call on the Tokio worker
        // (not `spawn_blocking`): on a local FS a stat is far cheaper than
        // blocking-pool dispatch, and it now runs at most once per window per
        // file. On a slow/network static root (NFS, sshfs) it briefly occupies
        // the worker thread — such deployments should keep revalidation off.
        let modified = {
            let mut guard = self.content.write();
            let entry = guard.entries.get_mut(key)?;
            match self.revalidate_ttl {
                Some(ttl) if entry.last_checked.elapsed() >= ttl => {
                    entry.last_checked = Instant::now(); // claim before releasing
                    entry.modified
                }
                // Another caller already revalidated this window — serve fresh
                // without a stat. (Reached for `None` only on a TOCTOU race
                // where revalidation was disabled meanwhile; serving is safe.)
                _ => return Some(Self::build_lookup(entry, headers, try_304, coding)),
            }
        };
        // stat() with no lock held — only the claiming caller reaches here.
        let disk_mtime = std::fs::metadata(key).ok().and_then(|m| m.modified().ok());
        if disk_mtime == Some(modified) {
            // Unchanged: the entry is already promoted and its window reset.
            let guard = self.content.read();
            guard
                .entries
                .peek(key)
                .map(|entry| Self::build_lookup(entry, headers, try_304, coding))
        } else {
            // Changed on disk: evict so the caller re-reads. Guard the pop on
            // the mtime we stat'd against so a concurrently-reinserted fresh
            // entry is not thrown away.
            let mut guard = self.content.write();
            if guard
                .entries
                .peek(key)
                .is_some_and(|e| e.modified == modified)
            {
                if let Some(evicted) = guard.entries.pop(key) {
                    guard.total_bytes -= evicted.footprint();
                }
            }
            None
        }
    }

    /// Check the cache for a path. Returns (file_type, was_cached).
    pub async fn check(&self, path: &str) -> (Option<FileType>, bool) {
        // Check cache — Mutex + peek (no LRU promotion, no allocation).
        {
            let guard = self.meta.lock();
            if let Some(&file_type) = guard.peek(path) {
                return (file_type, true);
            }
        }

        // Cache miss — async filesystem check
        let file_type = match tokio::fs::metadata(path).await {
            Ok(meta) if meta.is_file() => Some(FileType::File),
            Ok(meta) if meta.is_dir() => Some(FileType::Dir),
            _ => None,
        };

        // Insert into cache (LruCache handles O(1) eviction automatically)
        self.meta.lock().put(path.to_string(), file_type);

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

    /// Read-only check whether content is in the cache. No LRU update, no I/O.
    #[inline]
    pub fn content_cached(&self, key: &str) -> bool {
        self.content.read().entries.peek(key).is_some()
    }

    /// Test-only thin wrapper over [`lookup`] for standalone conditional checks.
    /// Returns `Some(true)` if cached and not modified, `Some(false)` if cached
    /// but modified, `None` on cache miss. Production code uses [`lookup`].
    ///
    /// [`lookup`]: Self::lookup
    #[cfg(test)]
    pub fn check_not_modified(&self, key: &str, headers: &HeaderMap) -> Option<bool> {
        match self.lookup(key, headers, true, None)? {
            Lookup::NotModified { .. } => Some(true),
            Lookup::Content { .. } => Some(false),
        }
    }

    /// Test-only thin wrapper over [`lookup`] returning just cached content and
    /// headers (no conditional check). Production code uses [`lookup`].
    ///
    /// [`lookup`]: Self::lookup
    #[cfg(test)]
    #[allow(clippy::type_complexity)]
    pub fn get_content(
        &self,
        key: &str,
    ) -> Option<(Bytes, Arc<str>, SystemTime, Arc<str>, Arc<str>)> {
        match self.lookup(key, &HeaderMap::new(), false, None)? {
            Lookup::Content {
                bytes,
                mime_type,
                modified,
                etag,
                last_modified_str,
                artifact: _,
            } => Some((bytes, mime_type, modified, etag, last_modified_str)),
            // try_304 = false never produces a NotModified result.
            Lookup::NotModified { .. } => None,
        }
    }

    /// Insert file content into the cache. Skips files larger than MAX_CACHE_FILE_SIZE.
    /// Evicts LRU entries when total cache size exceeds MAX_CACHE_TOTAL_BYTES.
    pub fn insert_content(
        &self,
        key: String,
        bytes: Bytes,
        mime_type: Arc<str>,
        modified: SystemTime,
        etag: Arc<str>,
        last_modified_str: Arc<str>,
    ) {
        if bytes.len() > MAX_CACHE_FILE_SIZE {
            return;
        }

        let mut guard = self.content.write();

        // Evict LRU entries while over budget — O(1) per eviction via pop_lru()
        while guard.total_bytes + bytes.len() > MAX_CACHE_TOTAL_BYTES {
            if let Some((_evicted_key, evicted)) = guard.entries.pop_lru() {
                guard.total_bytes -= evicted.footprint();
            } else {
                break;
            }
        }

        // Remove old entry if re-inserting same key
        if let Some(old) = guard.entries.pop(&key) {
            guard.total_bytes -= old.footprint();
        }

        guard.total_bytes += bytes.len();
        guard.entries.put(
            key,
            ContentEntry {
                bytes,
                mime_type,
                modified,
                etag,
                last_modified_str,
                last_checked: Instant::now(),
                artifacts: std::array::from_fn(|_| ArtifactState::Absent),
            },
        );
    }

    /// Claim the right to build `key`'s artifact for `coding`. `None` means
    /// another request is already building that one — the caller serves this
    /// request the per-request way and lets that one finish. Without the
    /// claim, a cold cache under load spends one top-of-range compression per
    /// concurrent request on the same file, all but one of them thrown away.
    pub fn claim_artifact(self: &Arc<Self>, key: &str, coding: Coding) -> Option<ArtifactClaim> {
        if !self
            .artifacts_in_flight
            .lock()
            .insert((key.to_string(), coding))
        {
            return None;
        }
        Some(ArtifactClaim {
            cache: Arc::clone(self),
            key: key.to_string(),
            coding,
        })
    }

    /// Attach `coding`'s artifact to the entry for `key`. The write is dropped
    /// unless the entry still describes `modified`: between the read that
    /// started the compression and its result the file may have changed on
    /// disk and the entry been replaced, and pairing new identity bytes with
    /// an artifact built from the old ones would serve two different
    /// representations under one validator.
    pub fn insert_artifact(
        &self,
        key: &str,
        coding: Coding,
        modified: SystemTime,
        artifact: Bytes,
    ) {
        let mut guard = self.content.write();
        // Promote first so the eviction loop below cannot pop the very entry
        // we are about to grow.
        match guard.entries.get(key) {
            Some(entry)
                if entry.modified == modified
                    && matches!(entry.artifacts[coding.slot()], ArtifactState::Absent) => {}
            _ => return,
        }
        while guard.total_bytes + artifact.len() > MAX_CACHE_TOTAL_BYTES {
            match guard.entries.pop_lru() {
                Some((evicted_key, evicted)) => {
                    guard.total_bytes -= evicted.footprint();
                    // Budget too small to hold this entry and its artifact:
                    // the entry itself came up for eviction. It is gone now,
                    // so there is nothing left to attach to.
                    if evicted_key == key {
                        return;
                    }
                }
                None => break,
            }
        }
        let len = artifact.len();
        if let Some(entry) = guard.entries.get_mut(key) {
            entry.artifacts[coding.slot()] = ArtifactState::Ready(artifact);
            guard.total_bytes += len;
        }
    }

    /// Record that `key`'s bytes do not compress under `coding`, so hits stop
    /// asking for that artifact. Same staleness guard as [`insert_artifact`]:
    /// a verdict about bytes the entry no longer holds is discarded.
    ///
    /// [`insert_artifact`]: Self::insert_artifact
    pub fn reject_artifact(&self, key: &str, coding: Coding, modified: SystemTime) {
        let mut guard = self.content.write();
        if let Some(entry) = guard.entries.get_mut(key) {
            if entry.modified == modified
                && matches!(entry.artifacts[coding.slot()], ArtifactState::Absent)
            {
                entry.artifacts[coding.slot()] = ArtifactState::Rejected;
            }
        }
    }

    /// Get a cached canonical path. Returns `None` on cache miss.
    /// The inner `Option<PathBuf>` distinguishes: `Some(path)` = canonicalization
    /// succeeded, `None` = file did not exist at cache time.
    pub fn get_canonical(&self, key: &str) -> Option<Option<PathBuf>> {
        self.canonical.lock().peek(key).cloned()
    }

    /// Cache a canonical path result. Uses the same capacity as the metadata cache.
    pub fn insert_canonical(&self, key: String, canonical: Option<PathBuf>) {
        // LruCache handles O(1) eviction automatically when at capacity.
        self.canonical.lock().put(key, canonical);
    }
}

/// Re-canonicalize a file path and verify it stays within the document root
/// or any allow-listed symlink target. Returns `false` if the path escapes
/// (TOCTOU mitigation).
async fn verify_canonical(
    file_path: &Path,
    canonical_root: &Path,
    allow_list: &crate::config::SymlinkAllowList,
) -> bool {
    match tokio::fs::canonicalize(file_path).await {
        Ok(real) => real.starts_with(canonical_root) || allow_list.allows(&real),
        Err(_) => false,
    }
}

/// Whole seconds since the Unix epoch (HTTP dates have second precision).
fn secs_since_epoch(t: &SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Generate a strong ETag from file size and modification time.
/// Strong (no `W/` prefix) so it can satisfy `If-Range`, which requires
/// strong comparison per RFC 9110 §13.1.5. Size + mtime fully identify the
/// byte content of a static file, matching nginx's static ETag semantics.
fn generate_etag(size: u64, modified: &SystemTime) -> String {
    let mtime_hex = secs_since_epoch(modified);
    format!("\"{size}-{mtime_hex:x}\"")
}

/// How a request's `Range` header maps onto a representation of `size` bytes.
#[derive(Debug, PartialEq, Eq)]
enum RangePlan {
    /// No applicable range — serve the full body with 200.
    Full,
    /// Serve bytes `start..=end` with 206 Partial Content.
    Partial { start: u64, end: u64 },
    /// Syntactically valid but unsatisfiable range — respond 416.
    NotSatisfiable,
}

/// Parse a range position: the RFC 9110 grammar allows only DIGIT, while
/// `u64::from_str` also accepts a leading `+` — reject that liberality so
/// `bytes=+5-+9` is ignored like any other malformed spec (nginx parity).
fn parse_range_pos(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// Parse a single-range `Range` header value (RFC 9110 §14.1.2) against a
/// representation of `size` bytes.
///
/// Returns `None` when the header must be ignored (unknown unit, multiple
/// ranges, malformed spec) — the caller serves the full body with 200.
/// Multiple ranges are deliberately not supported in this iteration;
/// ignoring them and returning the full representation is RFC-permitted.
fn parse_range(value: &str, size: u64) -> Option<RangePlan> {
    // Empty representation: serve the full (empty) 200 instead of 416 —
    // RFC 9110 permits either, nginx skips its range filter at zero length,
    // and probing clients (players, download managers sending `bytes=0-`)
    // handle an empty 200 more gracefully than a 416.
    if size == 0 {
        return None;
    }
    let (unit, spec) = value.split_once('=')?;
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return None;
    }
    let spec = spec.trim();
    if spec.contains(',') {
        return None; // multipart/byteranges not supported — serve full
    }

    if let Some(suffix) = spec.strip_prefix('-') {
        // suffix-range: last N bytes
        let n = parse_range_pos(suffix)?;
        if n == 0 {
            return Some(RangePlan::NotSatisfiable);
        }
        let start = size.saturating_sub(n);
        return Some(RangePlan::Partial {
            start,
            end: size - 1,
        });
    }

    let (first, last) = spec.split_once('-')?;
    let start = parse_range_pos(first)?;
    let last: Option<u64> = if last.is_empty() {
        None
    } else {
        Some(parse_range_pos(last)?)
    };
    // Syntactic validity before satisfiability: an inverted range is invalid
    // regardless of the representation size (RFC 9110 §14.1.1) — ignore the
    // header, same as any other malformed spec.
    if last.is_some_and(|l| l < start) {
        return None;
    }
    if start >= size {
        return Some(RangePlan::NotSatisfiable);
    }
    let end = last.map_or(size - 1, |l| l.min(size - 1));
    Some(RangePlan::Partial { start, end })
}

/// RFC 9110 §13.1.5: apply `Range` only when the `If-Range` validator matches.
/// ETag comparison is strong (weak validators never match); date comparison
/// is exact. Absent header means the range applies unconditionally.
fn if_range_matches(headers: &HeaderMap, etag: &str, modified: &SystemTime) -> bool {
    let Some(if_range) = headers.get(header::IF_RANGE) else {
        return true;
    };
    let Ok(value) = if_range.to_str() else {
        return false;
    };
    let value = value.trim();
    if value.starts_with("W/") {
        return false; // weak entity tag never strong-matches
    }
    if value.starts_with('"') {
        return value == etag;
    }
    // HTTP-date validator: exact match with Last-Modified (second precision).
    // RFC 9110 §13.1.5 only allows a date here when it is a strong validator,
    // and a Last-Modified is only provably strong (§8.8.2.2) once its second
    // has fully elapsed — a file written this second could change again
    // without moving the date, splicing fragments of the new bytes onto the
    // client's old prefix.
    match httpdate::parse_http_date(value) {
        Ok(if_range_time) => {
            let mtime = secs_since_epoch(modified);
            mtime == secs_since_epoch(&if_range_time)
                && mtime < secs_since_epoch(&SystemTime::now())
        }
        Err(_) => false,
    }
}

/// Decide how to serve a static representation given the request's range
/// headers. RFC 9110 §14.2 defines range handling for GET; HEAD is included
/// for nginx/Apache parity — §9.3.2 wants HEAD to mirror GET's headers, so
/// download managers probing resumability with `HEAD + Range` see the same
/// 206/Content-Range they would get from nginx (hyper elides the body).
fn plan_range(
    method: &http::Method,
    headers: &HeaderMap,
    size: u64,
    etag: &str,
    modified: &SystemTime,
) -> RangePlan {
    if method != http::Method::GET && method != http::Method::HEAD {
        return RangePlan::Full;
    }
    let mut range_lines = headers.get_all(header::RANGE).iter();
    let Some(range) = range_lines.next().and_then(|v| v.to_str().ok()) else {
        return RangePlan::Full;
    };
    // Multiple Range header lines are semantically one comma-joined list,
    // i.e. a multi-range request — fall back to the full response like any
    // other multi-range.
    if range_lines.next().is_some() {
        return RangePlan::Full;
    }
    if !if_range_matches(headers, etag, modified) {
        return RangePlan::Full;
    }
    parse_range(range, size).unwrap_or(RangePlan::Full)
}

/// Build a 416 Range Not Satisfiable response advertising the actual size.
fn build_416(size: u64) -> Result<Response<ResponseBody>, http::Error> {
    Response::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(header::CONTENT_RANGE, format!("bytes */{size}"))
        .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
        .body(full_body(Bytes::from_static(b"416 Range Not Satisfiable")))
}

/// Weak entity-tag comparison (RFC 9110 §8.8.3.2): tags match if their
/// opaque parts are equal, ignoring the `W/` weakness prefix on either side.
fn etag_weak_eq(a: &str, b: &str) -> bool {
    fn strip(t: &str) -> &str {
        t.strip_prefix("W/").unwrap_or(t)
    }
    strip(a) == strip(b)
}

/// Check if the request has matching conditional headers (If-None-Match or If-Modified-Since).
fn check_not_modified(headers: &HeaderMap, etag: &str, modified: &SystemTime) -> bool {
    // If-None-Match takes priority per RFC 7232 §3.3
    if let Some(inm) = headers.get(header::IF_NONE_MATCH) {
        if let Ok(val) = inm.to_str() {
            // Weak comparison: a client may hold a weakened tag (the
            // compression layer downgrades the ETag on encoded responses)
            // that must still revalidate against the strong original.
            return val.split(',').any(|tag| {
                let t = tag.trim();
                t == "*" || etag_weak_eq(t, etag)
            });
        }
    }

    // Fall back to If-Modified-Since
    if let Some(ims) = headers.get(header::IF_MODIFIED_SINCE) {
        if let Ok(val) = ims.to_str() {
            if let Ok(ims_time) = httpdate::parse_http_date(val) {
                // File not modified if mtime <= If-Modified-Since
                return secs_since_epoch(modified) <= secs_since_epoch(&ims_time);
            }
        }
    }

    false
}

/// ETag to send on a 304: if the client's matching `If-None-Match` tag was
/// weak, it cached a representation whose tag the compression layer
/// downgraded — echo the weak form. Answering with the strong tag would let
/// the cache "re-strengthen" the stored entry's validator (RFC 9111 §4.3.4
/// header update), re-opening the If-Range representation-mixing hole the
/// downgrade exists to close.
fn etag_for_304(headers: &HeaderMap, etag: &str) -> String {
    let client_tag_weak = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|val| {
            val.split(',').any(|tag| {
                let t = tag.trim();
                t.starts_with("W/") && etag_weak_eq(t, etag)
            })
        });
    if client_tag_weak {
        format!("W/{etag}")
    } else {
        etag.to_string()
    }
}

/// Build a 304 Not Modified response with caching headers.
fn build_304(etag: &str, last_modified_str: &str, cache_control: &str) -> Response<ResponseBody> {
    Response::builder()
        .status(StatusCode::NOT_MODIFIED)
        .header(header::ETAG, etag)
        .header(header::LAST_MODIFIED, last_modified_str)
        .header(header::CACHE_CONTROL, cache_control)
        .body(full_body(Bytes::new()))
        .unwrap()
}

/// Add caching headers to a response builder.
/// Note: `Vary: Accept-Encoding` is NOT added here — the compression layer appends it.
fn add_cache_headers(
    builder: http::response::Builder,
    etag: &str,
    last_modified_str: &str,
    cache_control: &str,
) -> http::response::Builder {
    builder
        .header(header::ETAG, etag)
        .header(header::LAST_MODIFIED, last_modified_str)
        .header(header::CACHE_CONTROL, cache_control)
}

/// Should range handling apply to a buffered response with this MIME type
/// and size? Disabled when the client accepts a content coding and the
/// representation would be served compressed instead: the client's stored copy
/// may be an encoded body that identity 206 fragments would corrupt, and
/// neither If-Range form can distinguish the representations (nginx clears
/// `allow_ranges` in its gzip filter for the same reason). Only buffered
/// responses are ever compressed — the streaming path is exempt.
fn ranges_allowed(will_encode: bool, levels: Levels, mime_type: &str, size: u64) -> bool {
    !(will_encode && crate::server::compression::would_compress(levels, mime_type, size))
}

/// Build a 200 or 206 response for a fully buffered body, honoring the
/// request's range plan. Used by both the content-cache hit path and the
/// small-file read path.
#[allow(clippy::too_many_arguments)]
fn respond_bytes(
    bytes: Bytes,
    mime_type: &str,
    etag: &str,
    last_modified_str: &str,
    modified: &SystemTime,
    cache_control: Option<&str>,
    method: &http::Method,
    request_headers: &HeaderMap,
    allow_ranges: bool,
    levels: Levels,
) -> Result<Response<ResponseBody>, http::Error> {
    let size = bytes.len() as u64;
    // A representation in the compression window is answered differently
    // depending on Accept-Encoding: encoded body with ranges disabled for
    // clients that accept a coding, identity with ranges (200/206/416) for the
    // rest. Shared caches must key on the header for every one of those
    // variants, including the identity ones no compression step ever touches.
    let varies_by_encoding = crate::server::compression::would_compress(levels, mime_type, size);
    let plan = if allow_ranges {
        plan_range(method, request_headers, size, etag, modified)
    } else {
        RangePlan::Full
    };
    let (status, body, content_range) = match plan {
        RangePlan::Full => (StatusCode::OK, bytes, None),
        RangePlan::Partial { start, end } => (
            StatusCode::PARTIAL_CONTENT,
            bytes.slice(start as usize..(end + 1) as usize),
            Some(format!("bytes {start}-{end}/{size}")),
        ),
        RangePlan::NotSatisfiable => {
            // A client that accepts a coding would have gotten a full 200
            // here (ranges disabled), so even the 416 varies by
            // Accept-Encoding.
            let mut response = build_416(size)?;
            if varies_by_encoding {
                response.headers_mut().insert(
                    header::VARY,
                    http::HeaderValue::from_static("Accept-Encoding"),
                );
            }
            return Ok(response);
        }
    };

    let mut builder = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, mime_type)
        .header(header::CONTENT_LENGTH, body.len());
    // Advertise ranges only when this request would actually honor them.
    // Compression removes the header too, but an encoder can decline to encode
    // (output not smaller) — relying on that removal would leave an identity
    // response advertising ranges the server just ignored.
    if allow_ranges {
        builder = builder.header(header::ACCEPT_RANGES, "bytes");
    }
    if varies_by_encoding {
        builder = builder.header(header::VARY, "Accept-Encoding");
    }
    if let Some(cr) = content_range {
        builder = builder.header(header::CONTENT_RANGE, cr);
    }
    if let Some(cc) = cache_control {
        builder = add_cache_headers(builder, etag, last_modified_str, cc);
    }
    builder.body(full_body(body))
}

/// Serve a static file with MIME type detection, content caching, and streaming.
///
/// Files ≤ 1 MiB are cached in memory. A larger file is streamed from disk,
/// unless it would be served compressed to this client and still fits the
/// compression window, in which case it is read whole for that request and
/// not cached.
/// Single-range `Range` requests (RFC 9110 §14) are honored for GET/HEAD with
/// 206/416; multi-range requests fall back to the full 200 response.
/// `coding` is the coding this client negotiated for stored copies: it selects
/// which artifact is served, and which one is asked for when none is stored
/// yet. When it is set and a buffered response would be served compressed,
/// range handling is disabled for it (see [`ranges_allowed`]);
/// a file that still streams is never compressed, so its ranges always work.
/// `levels` is what the server offers to anybody, which decides which
/// responses vary by `Accept-Encoding` — a question about the representation
/// rather than about this request.
/// Re-validates the file path at serve time against `canonical_root`
/// to mitigate TOCTOU symlink swap attacks.
#[allow(clippy::too_many_arguments)]
pub async fn serve(
    file_path: &Path,
    cache: &FileCache,
    canonical_root: &Path,
    allow_list: &crate::config::SymlinkAllowList,
    method: &http::Method,
    request_headers: &HeaderMap,
    cache_control: Option<&str>,
    coding: Option<Coding>,
    levels: Levels,
) -> Result<Response<ResponseBody>, crate::types::BoxError> {
    let cache_key = file_path.to_string_lossy();

    // Conditional (304) evaluation applies to GET/HEAD only: RFC 9110
    // §13.1.2-3 reserves 304 for those methods (other methods would get 412
    // semantics, which don't apply to a plain static fetch), and nginx
    // likewise skips not-modified handling for them.
    let conditional = method == http::Method::GET || method == http::Method::HEAD;

    // Combined 304 + content lookup: a single cache access (and at most one
    // stat, only when the revalidation window has elapsed) resolves both the
    // conditional check and the content fetch. 304 is only serviceable for a
    // conditional method with a Cache-Control to echo.
    let try_304 = conditional && cache_control.is_some();
    match cache.lookup(&cache_key, request_headers, try_304, coding) {
        Some(Lookup::NotModified {
            etag,
            last_modified_str,
        }) => {
            let cc = cache_control.expect("try_304 implies cache_control is Some");
            return Ok(build_304(
                &etag_for_304(request_headers, &etag),
                &last_modified_str,
                cc,
            ));
        }
        Some(Lookup::Content {
            bytes,
            mime_type,
            modified,
            etag,
            last_modified_str,
            artifact,
        }) => {
            let identity_len = bytes.len();
            let allow_ranges =
                ranges_allowed(coding.is_some(), levels, &mime_type, identity_len as u64);
            let mut response = respond_bytes(
                bytes.clone(),
                &mime_type,
                &etag,
                &last_modified_str,
                &modified,
                cache_control,
                method,
                request_headers,
                allow_ranges,
                levels,
            )?;
            // `allow_ranges` is false for exactly the representations the
            // compression layer would encode for this client, so a false here
            // means `respond_bytes` produced a full 200 and the body can be
            // swapped for the encoded one without disturbing a range plan.
            if let Some(coding) = coding.filter(|_| !allow_ranges) {
                match artifact {
                    ArtifactState::Ready(artifact) => {
                        let saved = identity_len.saturating_sub(artifact.len());
                        *response.body_mut() = full_body(artifact);
                        crate::server::compression::mark_encoded(&mut response, coding);
                        response.extensions_mut().insert(PrecompressedSaving(saved));
                    }
                    // No artifact yet: this request is served the per-request
                    // way and the caller is asked to build one for the next.
                    ArtifactState::Absent => {
                        response.extensions_mut().insert(ArtifactWanted {
                            key: cache_key.into_owned(),
                            coding,
                            bytes,
                            modified,
                        });
                    }
                    // These bytes do not compress — asking again would only
                    // repeat the work that established that.
                    ArtifactState::Rejected => {}
                }
            }
            return Ok(response);
        }
        None => {}
    }

    // Cache miss — compute MIME type
    let mime_type: Arc<str> = mime_guess::from_path(file_path)
        .first_or_octet_stream()
        .to_string()
        .into();

    // 2. TOCTOU mitigation: re-canonicalize before reading from disk.
    //    Skip the syscall if the canonical cache already validated this path
    //    (the routing layer's validate_path() populates this cache).
    let already_validated = cache.get_canonical(&cache_key).is_some_and(|opt| {
        opt.as_ref()
            .is_some_and(|p| p.starts_with(canonical_root) || allow_list.allows(p))
    });
    if !already_validated && !verify_canonical(file_path, canonical_root, allow_list).await {
        tracing::warn!(
            path = %file_path.display(),
            "TOCTOU: path escaped document root at serve time"
        );
        return Ok(Response::builder()
            .status(StatusCode::NOT_FOUND)
            .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
            .body(full_body(Bytes::from_static(b"404 Not Found")))?);
    }

    // 3. Open the file and take metadata from the handle so that size,
    //    mtime, ETag, and the bytes served all describe the same inode.
    //    A stat() on the path followed by a separate open() would let a
    //    concurrent deploy swap the file in between, silently pairing new
    //    bytes with the old validator and mis-slicing Range responses.
    let mut file = match tokio::fs::File::open(file_path).await {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Ok(Response::builder()
                .status(StatusCode::NOT_FOUND)
                .header(header::CONTENT_TYPE, "text/plain; charset=utf-8")
                .body(full_body(Bytes::from_static(b"404 Not Found")))?);
        }
        Err(e) => return Err(e.into()),
    };
    let metadata = file.metadata().await?;

    let file_size = metadata.len();
    let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let etag_str = generate_etag(file_size, &modified);
    let last_modified_str = httpdate::fmt_http_date(modified);

    // Check for 304 before reading file content (no Arc allocation yet)
    if conditional {
        if let Some(cc) = cache_control {
            if check_not_modified(request_headers, &etag_str, &modified) {
                return Ok(build_304(
                    &etag_for_304(request_headers, &etag_str),
                    &last_modified_str,
                    cc,
                ));
            }
        }
    }

    // Allocate Arc<str> only after 304 check passes
    let etag: Arc<str> = etag_str.as_str().into();
    let last_modified_arc: Arc<str> = last_modified_str.as_str().into();

    // 4. Read the file in full and answer from memory. Files up to the cache
    //    limit are also kept; a larger one is read only when the alternative
    //    is sending it uncompressed, because the compression layer encodes a
    //    body it holds whole and passes a stream through. Without this a file
    //    between the cache limit and the top of the compression window — a
    //    bundle, a source map, a WASM module — went out several times its
    //    compressed size, while a PHP response of the same size did not.
    //    The read is capped at the fstat'ed size so a file growing mid-read
    //    cannot produce a body longer than the validator describes.
    //    Only so many such files are read at once: the bytes stay resident
    //    until the response has been written, and the connection limit alone is
    //    far too generous a ceiling for them. Over the cap the file streams.
    let off_disk = (file_size > MAX_CACHE_FILE_SIZE as u64
        && coding.is_some()
        && crate::server::compression::would_compress(levels, &mime_type, file_size))
    .then(|| {
        Arc::clone(&cache.off_disk_compression)
            .try_acquire_owned()
            .ok()
            .map(Arc::new)
    })
    .flatten();
    if file_size <= MAX_CACHE_FILE_SIZE as u64 || off_disk.is_some() {
        use tokio::io::AsyncReadExt;
        let mut contents = Vec::with_capacity(file_size as usize);
        (&mut file)
            .take(file_size)
            .read_to_end(&mut contents)
            .await?;

        let bytes = Bytes::from(contents);
        // File truncated between fstat and the read: the validator no longer
        // describes these bytes. Serve them as an uncacheable full response —
        // no ETag/Last-Modified that would mislabel the body downstream, no
        // range slicing against an uncertain size, no cache insert. The next
        // request re-fstats and heals.
        if bytes.len() as u64 != file_size {
            return Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, &*mime_type)
                .header(header::CONTENT_LENGTH, bytes.len())
                .body(full_body(bytes))?);
        }
        // Refused for anything above the cache limit, which is the point:
        // a file read only to be compressed must not evict the small files
        // the cache exists for, nor be re-served from a budget it would fill
        // on its own.
        cache.insert_content(
            cache_key.into_owned(),
            bytes.clone(),
            mime_type.clone(),
            modified,
            etag.clone(),
            last_modified_arc.clone(),
        );

        let allow_ranges = ranges_allowed(coding.is_some(), levels, &mime_type, file_size);
        let mut response = respond_bytes(
            bytes,
            &mime_type,
            &etag,
            &last_modified_str,
            &modified,
            cache_control,
            method,
            request_headers,
            allow_ranges,
            levels,
        )?;
        if let Some(permit) = off_disk {
            response.extensions_mut().insert(OffDiskCompression(permit));
        }
        return Ok(response);
    }

    // 5. Large file: stream from the already-open handle. These bytes are not
    //    compressed — too large for the window, a type that does not compress,
    //    a client that accepts no coding, or no permit free to read it whole —
    //    so ranges are always safe here.
    //
    //    A file inside the compression window still varies by Accept-Encoding
    //    even though it is being streamed: the same URL answers a client that
    //    negotiated a coding with a compressed body and no ranges, and a shared
    //    cache that stored this response under the bare URL would hand these
    //    full-size bytes to that client. The compression layer cannot declare
    //    it downstream — a stream has no length for it to weigh — so it is
    //    declared here, where the file's size is known.
    let varies_by_encoding =
        crate::server::compression::would_compress(levels, &mime_type, file_size);
    let range_plan = plan_range(method, request_headers, file_size, &etag, &modified);
    if matches!(range_plan, RangePlan::NotSatisfiable) {
        // A client that negotiated a coding would have been answered 200 with
        // the whole (compressed) file, so even the 416 varies.
        let mut response = build_416(file_size)?;
        if varies_by_encoding {
            response.headers_mut().insert(
                header::VARY,
                http::HeaderValue::from_static("Accept-Encoding"),
            );
        }
        return Ok(response);
    }

    let mut builder = Response::builder()
        .header(header::CONTENT_TYPE, &*mime_type)
        .header(header::ACCEPT_RANGES, "bytes");
    if varies_by_encoding {
        builder = builder.header(header::VARY, "Accept-Encoding");
    }

    // Range request: seek to the start and cap the read at the range length.
    let body_len = if let RangePlan::Partial { start, end } = range_plan {
        use tokio::io::AsyncSeekExt;
        file.seek(io::SeekFrom::Start(start)).await?;
        builder = builder.status(StatusCode::PARTIAL_CONTENT).header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{file_size}"),
        );
        end - start + 1
    } else {
        builder = builder.status(StatusCode::OK);
        file_size
    };
    // Unlike the buffered path, which bails when the read comes up short
    // (file truncated between fstat and read), Content-Length here is committed
    // to the wire before the body streams. A concurrent truncation therefore
    // cannot be caught in time: the stream ends short and hyper aborts the
    // connection. That abort is the correct signal for an unfulfillable
    // Content-Length, and it is identical to the pre-existing full-response
    // streaming behavior (and to nginx/Apache). The same-inode open above keeps
    // an atomic rename-swap deploy from hitting this; only an in-place truncate
    // of the open file can, which a normal deploy does not do.
    builder = builder.header(header::CONTENT_LENGTH, body_len);

    // 64KB read buffer for large file streaming (default is 4KB).
    // Reduces read syscalls by ~16x for typical large static files.
    let stream =
        ReaderStream::with_capacity(tokio::io::AsyncReadExt::take(file, body_len), 64 * 1024);
    let stream_body =
        StreamBody::new(stream.map(|result: Result<Bytes, io::Error>| result.map(Frame::data)));

    if let Some(cc) = cache_control {
        builder = add_cache_headers(builder, &etag, &last_modified_str, cc);
    }

    Ok(builder.body(BodyExt::boxed(stream_body))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SymlinkAllowList;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use tempfile::TempDir;

    fn empty_allow() -> SymlinkAllowList {
        SymlinkAllowList::default()
    }

    /// The shipped configuration: every coding offered.
    fn offered() -> Levels {
        Levels {
            brotli: 5,
            gzip: 6,
            zstd: 6,
        }
    }

    /// Compression switched off entirely, as `COMPRESSION_ENCODINGS=off`
    /// leaves it.
    fn no_codings() -> Levels {
        Levels::default()
    }

    /// Canonicalize the temp dir path so it matches what `verify_canonical` resolves
    /// (e.g. macOS `/var` → `/private/var` symlink).
    fn canonical_root(dir: &TempDir) -> PathBuf {
        std::fs::canonicalize(dir.path()).unwrap()
    }

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

        let meta = cache.meta.lock();
        assert!(meta.len() <= 2);
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

        // Insert f3 — should evict f1 (oldest insertion, since check() uses
        // peek() without LRU promotion to avoid write-lock contention)
        cache.check(&f3.to_string_lossy()).await;

        let meta = cache.meta.lock();
        assert!(!meta.contains(&f1.to_string_lossy().to_string()));
        assert!(meta.contains(&f2.to_string_lossy().to_string()));
        assert!(meta.contains(&f3.to_string_lossy().to_string()));
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
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &HeaderMap::new(),
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
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
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &HeaderMap::new(),
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
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
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &HeaderMap::new(),
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
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
        let dir = TempDir::new().unwrap();
        let cache = FileCache::new(10);
        let response = serve(
            &dir.path().join("nonexistent.txt"),
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &HeaderMap::new(),
            Some("public, max-age=86400"),
            None,
            offered(),
        )
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

    fn test_modified() -> SystemTime {
        SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)
    }

    fn test_etag() -> Arc<str> {
        generate_etag(100, &test_modified()).into()
    }

    fn test_last_modified_str() -> Arc<str> {
        httpdate::fmt_http_date(test_modified()).into()
    }

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
            test_modified(),
            test_etag(),
            test_last_modified_str(),
        );
        let hit = cache.get_content("/foo.txt");
        assert!(hit.is_some());
        let (bytes, mime, _, _, _) = hit.unwrap();
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
            test_modified(),
            test_etag(),
            test_last_modified_str(),
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
            cache.insert_content(
                format!("file_{}", i),
                data.clone(),
                mime.clone(),
                test_modified(),
                test_etag(),
                test_last_modified_str(),
            );
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
            test_modified(),
            test_etag(),
            test_last_modified_str(),
        );

        // First entry should be evicted
        assert!(cache.get_content("file_0").is_none());
        assert!(cache.get_content("overflow").is_some());
    }

    #[test]
    fn test_revalidation_promotes_hot_entry_on_access() {
        // In revalidation mode a cache hit must promote the entry to MRU,
        // otherwise a constantly-accessed hot file is evicted ahead of colder,
        // later-inserted ones once the byte budget fills.
        let dir = TempDir::new().unwrap();
        // Zero-TTL → every lookup takes the revalidating slow path that promotes.
        let cache = FileCache::with_revalidation_ttl(10, Some(std::time::Duration::ZERO));

        // `Bytes::clone` is Arc-shared, so all entries share one physical 1 MiB
        // buffer; the byte budget is accounted by len(), filling it cheaply.
        let data = Bytes::from(vec![0u8; MAX_CACHE_FILE_SIZE]);
        let mime: Arc<str> = "application/octet-stream".into();
        let entries = MAX_CACHE_TOTAL_BYTES / MAX_CACHE_FILE_SIZE;

        let insert = |key: &str, modified: SystemTime| {
            cache.insert_content(
                key.to_string(),
                data.clone(),
                mime.clone(),
                modified,
                generate_etag(1, &modified).as_str().into(),
                httpdate::fmt_http_date(modified).as_str().into(),
            );
        };

        let mut keys = Vec::new();
        for i in 0..entries {
            // Tiny on-disk file — only its mtime is read during revalidation.
            let path = dir.path().join(format!("hot_{i}.bin"));
            fs::write(&path, b"x").unwrap();
            let modified = fs::metadata(&path).unwrap().modified().unwrap();
            let key = path.to_string_lossy().to_string();
            insert(&key, modified);
            keys.push(key);
        }

        // Access the oldest (LRU) entry — this must promote it to MRU.
        assert!(cache.get_content(&keys[0]).is_some());

        // One more insert pushes over budget, evicting the true LRU entry.
        let overflow = dir.path().join("overflow.bin");
        fs::write(&overflow, b"x").unwrap();
        let ov_modified = fs::metadata(&overflow).unwrap().modified().unwrap();
        insert(&overflow.to_string_lossy(), ov_modified);

        // keys[0] was promoted, so keys[1] (now LRU) is evicted in its place.
        assert!(
            cache.get_content(&keys[0]).is_some(),
            "accessed (promoted) entry must survive eviction"
        );
        assert!(
            cache.get_content(&keys[1]).is_none(),
            "the now-LRU entry should be evicted instead of the hot one"
        );
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
        let _response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &HeaderMap::new(),
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
        let cached = cache.get_content(&cache_key);
        assert!(cached.is_some());
        let (bytes, mime, _, _, _) = cached.unwrap();
        assert_eq!(bytes, &b"cached content"[..]);
        assert!(mime.contains("text/plain"));

        // Second serve should hit cache (we can't directly assert cache hit,
        // but we verify it still works)
        let response2 = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &HeaderMap::new(),
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
        assert_eq!(response2.status(), StatusCode::OK);
    }

    // --- HTTP caching tests ---

    #[test]
    fn test_generate_etag_deterministic() {
        let modified = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(0x65a1b2c3);
        let etag1 = generate_etag(1024, &modified);
        let etag2 = generate_etag(1024, &modified);
        assert_eq!(etag1, etag2);
        assert!(etag1.starts_with('"'), "static ETag must be strong");
        assert!(etag1.ends_with('"'));
        assert_eq!(etag1, "\"1024-65a1b2c3\"");
    }

    #[test]
    fn test_generate_etag_varies_with_size() {
        let modified = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
        assert_ne!(generate_etag(100, &modified), generate_etag(200, &modified));
    }

    #[test]
    fn test_generate_etag_varies_with_mtime() {
        let m1 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1000);
        let m2 = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(2000);
        assert_ne!(generate_etag(100, &m1), generate_etag(100, &m2));
    }

    #[test]
    fn test_check_not_modified_etag_match() {
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        assert!(check_not_modified(&headers, &etag, &modified));
    }

    #[test]
    fn test_check_not_modified_etag_mismatch() {
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, "W/\"wrong\"".parse().unwrap());
        assert!(!check_not_modified(&headers, &etag, &modified));
    }

    #[test]
    fn test_check_not_modified_weak_client_tag_matches_strong() {
        // A client that received a compressed response holds the weakened
        // tag W/"…" and must still revalidate to 304 (weak comparison).
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, format!("W/{etag}").parse().unwrap());
        assert!(check_not_modified(&headers, &etag, &modified));
    }

    #[test]
    fn test_check_not_modified_etag_star() {
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, "*".parse().unwrap());
        assert!(check_not_modified(&headers, &etag, &modified));
    }

    #[test]
    fn test_check_not_modified_if_modified_since() {
        let modified = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let etag = generate_etag(100, &modified);
        let date_str = httpdate::fmt_http_date(modified);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_MODIFIED_SINCE, date_str.parse().unwrap());
        assert!(check_not_modified(&headers, &etag, &modified));
    }

    #[test]
    fn test_check_not_modified_if_modified_since_older() {
        let modified = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000);
        let older = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_699_999_000);
        let etag = generate_etag(100, &modified);
        let date_str = httpdate::fmt_http_date(older);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_MODIFIED_SINCE, date_str.parse().unwrap());
        assert!(!check_not_modified(&headers, &etag, &modified));
    }

    #[test]
    fn test_check_not_modified_no_headers() {
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        let headers = HeaderMap::new();
        assert!(!check_not_modified(&headers, &etag, &modified));
    }

    #[test]
    fn test_check_not_modified_etag_priority_over_ims() {
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        let date_str = httpdate::fmt_http_date(modified);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, "W/\"wrong\"".parse().unwrap());
        headers.insert(header::IF_MODIFIED_SINCE, date_str.parse().unwrap());
        // If-None-Match is checked first and doesn't match
        assert!(!check_not_modified(&headers, &etag, &modified));
    }

    #[tokio::test]
    async fn test_serve_returns_cache_headers() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("style.css");
        fs::write(&file_path, "body {}").unwrap();

        let cache = FileCache::new(10);
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &HeaderMap::new(),
            Some("public, max-age=3600"),
            None,
            offered(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let cc = response
            .headers()
            .get(header::CACHE_CONTROL)
            .unwrap()
            .to_str()
            .unwrap();
        assert_eq!(cc, "public, max-age=3600");
        assert!(response.headers().get(header::ETAG).is_some());
        assert!(response.headers().get(header::LAST_MODIFIED).is_some());
        // Vary: Accept-Encoding is added by the compression layer, not by static file serving
        assert!(response.headers().get(header::VARY).is_none());
    }

    #[tokio::test]
    async fn test_serve_no_cache_headers_when_disabled() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("style.css");
        fs::write(&file_path, "body {}").unwrap();

        let cache = FileCache::new(10);
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &HeaderMap::new(),
            None,
            None,
            offered(),
        )
        .await
        .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::CACHE_CONTROL).is_none());
        assert!(response.headers().get(header::ETAG).is_none());
        assert!(response.headers().get(header::LAST_MODIFIED).is_none());
    }

    #[tokio::test]
    async fn test_serve_304_with_matching_etag() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "hello").unwrap();

        let cache = FileCache::new(10);

        // First request to populate cache and get ETag
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &HeaderMap::new(),
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
        let etag = response
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Second request with matching If-None-Match
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &headers,
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    }

    #[tokio::test]
    async fn test_serve_200_with_wrong_etag() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "hello").unwrap();

        let cache = FileCache::new(10);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, "W/\"wrong\"".parse().unwrap());
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &headers,
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_serve_304_from_cache_hit() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("cached.txt");
        fs::write(&file_path, "content").unwrap();

        let cache = FileCache::new(10);

        // Populate cache
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &HeaderMap::new(),
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
        let etag = response
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        // Hit cache with matching ETag -> 304
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &headers,
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert!(response.headers().get(header::ETAG).is_some());
        assert!(response.headers().get(header::CACHE_CONTROL).is_some());
    }

    #[test]
    fn test_content_cache_revalidation_detects_mtime_change() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("revalidate.txt");
        fs::write(&file_path, "original").unwrap();

        // Zero-TTL revalidation stats on every hit, so the change is detected
        // immediately without waiting out a window.
        let cache = FileCache::with_revalidation_ttl(10, Some(std::time::Duration::ZERO));
        let key = file_path.to_string_lossy().to_string();
        let modified = fs::metadata(&file_path).unwrap().modified().unwrap();

        cache.insert_content(
            key.clone(),
            Bytes::from_static(b"original"),
            "text/plain".into(),
            modified,
            generate_etag(8, &modified).as_str().into(),
            httpdate::fmt_http_date(modified).as_str().into(),
        );

        // Cache hit before modification
        assert!(cache.get_content(&key).is_some());

        // Modify file on disk (touch with new mtime)
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&file_path, "updated").unwrap();

        // Cache should detect mtime change and return None
        assert!(
            cache.get_content(&key).is_none(),
            "Revalidation should detect mtime change and evict stale entry"
        );
    }

    #[test]
    fn test_content_cache_revalidation_window_keeps_entry() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("windowed.txt");
        fs::write(&file_path, "original").unwrap();

        // Default `on` TTL (3s) — a change within the window stays masked until
        // the window elapses, trading bounded staleness for far fewer stats.
        let cache = FileCache::with_revalidation(10, true);
        let key = file_path.to_string_lossy().to_string();
        let modified = fs::metadata(&file_path).unwrap().modified().unwrap();

        cache.insert_content(
            key.clone(),
            Bytes::from_static(b"original"),
            "text/plain".into(),
            modified,
            generate_etag(8, &modified).as_str().into(),
            httpdate::fmt_http_date(modified).as_str().into(),
        );

        // Modify on disk, then read back immediately — still within the window.
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&file_path, "updated").unwrap();

        assert!(
            cache.get_content(&key).is_some(),
            "Within the revalidation window the entry should be served without a stat-driven eviction"
        );
    }

    #[test]
    fn test_content_cache_no_revalidation_when_disabled() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("no_revalidate.txt");
        fs::write(&file_path, "original").unwrap();

        let cache = FileCache::new(10); // revalidation off (default)
        let key = file_path.to_string_lossy().to_string();
        let modified = fs::metadata(&file_path).unwrap().modified().unwrap();

        cache.insert_content(
            key.clone(),
            Bytes::from_static(b"original"),
            "text/plain".into(),
            modified,
            generate_etag(8, &modified).as_str().into(),
            httpdate::fmt_http_date(modified).as_str().into(),
        );

        // Modify file on disk
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&file_path, "updated").unwrap();

        // Cache should still return cached content (no revalidation)
        assert!(
            cache.get_content(&key).is_some(),
            "Without revalidation, cache should return stale content"
        );
    }

    #[test]
    fn test_check_not_modified_revalidation_detects_change() {
        use http::header;

        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("check_304.txt");
        fs::write(&file_path, "hello").unwrap();

        let cache = FileCache::with_revalidation_ttl(10, Some(std::time::Duration::ZERO));
        let key = file_path.to_string_lossy().to_string();
        let modified = fs::metadata(&file_path).unwrap().modified().unwrap();
        let etag: Arc<str> = generate_etag(5, &modified).as_str().into();

        cache.insert_content(
            key.clone(),
            Bytes::from_static(b"hello"),
            "text/plain".into(),
            modified,
            etag.clone(),
            httpdate::fmt_http_date(modified).as_str().into(),
        );

        // Before modification: should find cached entry
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        assert!(cache.check_not_modified(&key, &headers).is_some());

        // Modify file
        std::thread::sleep(std::time::Duration::from_millis(50));
        fs::write(&file_path, "changed").unwrap();

        // After modification: should return None (cache miss)
        assert!(
            cache.check_not_modified(&key, &headers).is_none(),
            "check_not_modified should detect mtime change and evict"
        );
    }

    // --- Range request tests ---

    #[test]
    fn test_parse_range_basic() {
        assert_eq!(
            parse_range("bytes=0-4", 100),
            Some(RangePlan::Partial { start: 0, end: 4 })
        );
        assert_eq!(
            parse_range("bytes=10-", 100),
            Some(RangePlan::Partial { start: 10, end: 99 })
        );
        assert_eq!(
            parse_range("bytes=-5", 100),
            Some(RangePlan::Partial { start: 95, end: 99 })
        );
    }

    #[test]
    fn test_parse_range_clamps_end_to_eof() {
        assert_eq!(
            parse_range("bytes=50-1000", 100),
            Some(RangePlan::Partial { start: 50, end: 99 })
        );
    }

    #[test]
    fn test_parse_range_suffix_larger_than_file() {
        // Suffix longer than the file → entire representation as 206
        assert_eq!(
            parse_range("bytes=-500", 100),
            Some(RangePlan::Partial { start: 0, end: 99 })
        );
    }

    #[test]
    fn test_parse_range_unsatisfiable() {
        // Start at/past EOF
        assert_eq!(
            parse_range("bytes=100-", 100),
            Some(RangePlan::NotSatisfiable)
        );
        assert_eq!(
            parse_range("bytes=200-300", 100),
            Some(RangePlan::NotSatisfiable)
        );
        // Zero-length suffix
        assert_eq!(
            parse_range("bytes=-0", 100),
            Some(RangePlan::NotSatisfiable)
        );
    }

    #[test]
    fn test_parse_range_empty_file_serves_full() {
        // Any Range against a 0-byte file is ignored — full (empty) 200,
        // matching nginx, which skips its range filter at zero length.
        assert_eq!(parse_range("bytes=0-", 0), None);
        assert_eq!(parse_range("bytes=-5", 0), None);
        assert_eq!(parse_range("bytes=0-4", 0), None);
    }

    #[test]
    fn test_parse_range_ignored() {
        // Unknown unit
        assert_eq!(parse_range("items=0-4", 100), None);
        // Multiple ranges — not supported, serve full
        assert_eq!(parse_range("bytes=0-4,10-14", 100), None);
        // Malformed specs
        assert_eq!(parse_range("bytes=", 100), None);
        assert_eq!(parse_range("bytes=abc-def", 100), None);
        assert_eq!(parse_range("bytes=5", 100), None);
        // Inverted range is syntactically invalid (RFC 9110 §14.1.1) and is
        // ignored regardless of where it sits relative to the file size —
        // validity is checked before satisfiability.
        assert_eq!(parse_range("bytes=10-5", 100), None);
        assert_eq!(parse_range("bytes=200-100", 100), None);
        // The grammar allows only DIGIT — a leading `+` (accepted by
        // u64::from_str) must be rejected like any malformed spec.
        assert_eq!(parse_range("bytes=+5-+9", 100), None);
        assert_eq!(parse_range("bytes=-+5", 100), None);
    }

    #[test]
    fn test_parse_range_single_byte() {
        assert_eq!(
            parse_range("bytes=0-0", 1),
            Some(RangePlan::Partial { start: 0, end: 0 })
        );
    }

    #[test]
    fn test_if_range_etag_strong_match() {
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_RANGE, etag.parse().unwrap());
        assert!(if_range_matches(&headers, &etag, &modified));
    }

    #[test]
    fn test_if_range_etag_mismatch() {
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_RANGE, "\"other\"".parse().unwrap());
        assert!(!if_range_matches(&headers, &etag, &modified));
    }

    #[test]
    fn test_if_range_weak_etag_never_matches() {
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        let weak = format!("W/{etag}");
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_RANGE, weak.parse().unwrap());
        assert!(!if_range_matches(&headers, &etag, &modified));
    }

    #[test]
    fn test_if_range_date_exact_match() {
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        let date = httpdate::fmt_http_date(modified);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_RANGE, date.parse().unwrap());
        assert!(if_range_matches(&headers, &etag, &modified));

        // A different date must not match (exact comparison, not <=)
        let older = httpdate::fmt_http_date(modified - std::time::Duration::from_secs(60));
        headers.insert(header::IF_RANGE, older.parse().unwrap());
        assert!(!if_range_matches(&headers, &etag, &modified));
    }

    #[test]
    fn test_if_range_date_fresh_mtime_not_strong() {
        // A Last-Modified whose second has not yet elapsed is a weak
        // validator (RFC 9110 §8.8.2.2): the file could change again within
        // that second without moving the date. The +1s mtime stands in for a
        // freshly written file at evaluation time (and covers future clock
        // skew) without racing the wall clock.
        let modified = SystemTime::now() + std::time::Duration::from_secs(1);
        let etag = generate_etag(100, &modified);
        let date = httpdate::fmt_http_date(modified);
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_RANGE, date.parse().unwrap());
        assert!(!if_range_matches(&headers, &etag, &modified));
    }

    #[test]
    fn test_if_range_absent_applies_range() {
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        assert!(if_range_matches(&HeaderMap::new(), &etag, &modified));
    }

    #[test]
    fn test_plan_range_method_gating() {
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, "bytes=0-4".parse().unwrap());

        assert_eq!(
            plan_range(&http::Method::GET, &headers, 100, &etag, &modified),
            RangePlan::Partial { start: 0, end: 4 }
        );
        // HEAD mirrors GET's headers (nginx/Apache parity) — hyper elides the body
        assert_eq!(
            plan_range(&http::Method::HEAD, &headers, 100, &etag, &modified),
            RangePlan::Partial { start: 0, end: 4 }
        );
        assert_eq!(
            plan_range(&http::Method::POST, &headers, 100, &etag, &modified),
            RangePlan::Full
        );
    }

    #[test]
    fn test_plan_range_multiple_header_lines_full() {
        // Two Range lines are semantically one multi-range list — serve full.
        let modified = test_modified();
        let etag = generate_etag(100, &modified);
        let mut headers = HeaderMap::new();
        headers.append(header::RANGE, "bytes=0-0".parse().unwrap());
        headers.append(header::RANGE, "bytes=5-9".parse().unwrap());
        assert_eq!(
            plan_range(&http::Method::GET, &headers, 100, &etag, &modified),
            RangePlan::Full
        );
    }

    /// Serve as a client that accepts Brotli — the coding the artifact tests
    /// use unless they are about the choice between codings.
    async fn serve_br(
        file_path: &std::path::Path,
        dir: &TempDir,
        cache: &FileCache,
    ) -> Response<ResponseBody> {
        serve_as(file_path, dir, cache, Coding::Br).await
    }

    /// Serve as a client that negotiated `coding`.
    async fn serve_as(
        file_path: &std::path::Path,
        dir: &TempDir,
        cache: &FileCache,
        coding: Coding,
    ) -> Response<ResponseBody> {
        serve(
            file_path,
            cache,
            &canonical_root(dir),
            &empty_allow(),
            &http::Method::GET,
            &HeaderMap::new(),
            Some("public, max-age=86400"),
            Some(coding),
            offered(),
        )
        .await
        .unwrap()
    }

    /// Run the background job the response asked for, inline.
    fn build_wanted_artifact(cache: &FileCache, wanted: &ArtifactWanted) {
        let artifact = crate::server::compression::compress_artifact(&wanted.bytes, wanted.coding)
            .expect("css compresses");
        cache.insert_artifact(
            &wanted.key,
            wanted.coding,
            wanted.modified,
            Bytes::from(artifact),
        );
    }

    fn wanted_by(response: &Response<ResponseBody>) -> ArtifactWanted {
        response
            .extensions()
            .get::<ArtifactWanted>()
            .expect("a cached compressible entry with no artifact asks for one")
            .clone()
    }

    /// A body that is both compressible and over the compression floor.
    fn css_body() -> String {
        "body { color: rebeccapurple; margin: 0 }\n".repeat(40)
    }

    #[tokio::test]
    async fn test_artifact_asked_for_on_a_cache_hit_then_served() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("app.css");
        let body = css_body();
        fs::write(&file_path, &body).unwrap();
        let cache = Arc::new(FileCache::new(10));
        let key = file_path.to_string_lossy().to_string();

        // First hit reads from disk and fills the content cache. Nothing is
        // asked for yet: a file served exactly once should not cost a q11
        // compression, so the ask waits for evidence the file is hot.
        let first = serve_br(&file_path, &dir, &cache).await;
        assert!(first.headers().get(header::CONTENT_ENCODING).is_none());
        assert!(first.extensions().get::<ArtifactWanted>().is_none());

        // Second hit comes out of the cache and asks for the artifact, still
        // serving identity bytes itself.
        let second = serve_br(&file_path, &dir, &cache).await;
        assert!(second.headers().get(header::CONTENT_ENCODING).is_none());
        let wanted = second
            .extensions()
            .get::<ArtifactWanted>()
            .expect("a cached compressible entry with no artifact asks for one")
            .clone();
        assert_eq!(wanted.key, key);
        assert_eq!(&wanted.bytes[..], body.as_bytes());

        // Stand in for the background job.
        let artifact = crate::server::compression::compress_artifact(&wanted.bytes, wanted.coding)
            .expect("css compresses");
        let artifact_len = artifact.len();
        assert!(artifact_len < body.len());
        cache.insert_artifact(
            &wanted.key,
            wanted.coding,
            wanted.modified,
            Bytes::from(artifact),
        );

        // Third hit is served from the artifact.
        let third = serve_br(&file_path, &dir, &cache).await;
        assert_eq!(third.headers()[header::CONTENT_ENCODING], "br");
        assert_eq!(
            third.headers()[header::CONTENT_LENGTH],
            artifact_len.to_string()
        );
        // Byte offsets do not survive the re-encoding, and the validator no
        // longer describes the bytes on the wire strongly.
        assert!(third.headers().get(header::ACCEPT_RANGES).is_none());
        assert!(third.headers()[header::ETAG]
            .to_str()
            .unwrap()
            .starts_with("W/"));
        assert!(third
            .headers()
            .get_all(header::VARY)
            .iter()
            .any(|v| v.to_str().unwrap().eq_ignore_ascii_case("accept-encoding")));
        assert_eq!(
            third.extensions().get::<PrecompressedSaving>().unwrap().0,
            body.len() - artifact_len
        );
        assert!(third.extensions().get::<ArtifactWanted>().is_none());
        assert_eq!(body_bytes(third).await.len(), artifact_len);
    }

    #[tokio::test]
    async fn test_gzip_client_gets_a_gzip_artifact() {
        use std::io::Read;

        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("app.css");
        let body = css_body();
        fs::write(&file_path, &body).unwrap();
        let cache = Arc::new(FileCache::new(10));

        serve_as(&file_path, &dir, &cache, Coding::Gzip).await;
        let second = serve_as(&file_path, &dir, &cache, Coding::Gzip).await;
        let wanted = wanted_by(&second);
        assert_eq!(wanted.coding, Coding::Gzip);
        build_wanted_artifact(&cache, &wanted);

        let third = serve_as(&file_path, &dir, &cache, Coding::Gzip).await;
        assert_eq!(third.headers()[header::CONTENT_ENCODING], "gzip");
        assert!(third.extensions().get::<ArtifactWanted>().is_none());

        // The stored bytes are a gzip stream, not brotli wearing a gzip label.
        let served = body_bytes(third).await;
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(&served[..])
            .read_to_end(&mut decoded)
            .unwrap();
        assert_eq!(decoded, body.as_bytes());
    }

    #[tokio::test]
    async fn test_one_codings_artifact_is_never_served_to_another() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("app.css");
        fs::write(&file_path, css_body()).unwrap();
        let cache = Arc::new(FileCache::new(10));

        // Give the entry a brotli artifact.
        serve_br(&file_path, &dir, &cache).await;
        let second = serve_br(&file_path, &dir, &cache).await;
        build_wanted_artifact(&cache, &wanted_by(&second));

        // A gzip client must not be handed those bytes: it would decode them
        // as gzip and fail. It gets identity, and asks for its own artifact.
        let gzip_hit = serve_as(&file_path, &dir, &cache, Coding::Gzip).await;
        assert!(gzip_hit.headers().get(header::CONTENT_ENCODING).is_none());
        assert_eq!(wanted_by(&gzip_hit).coding, Coding::Gzip);

        // Building it leaves the brotli artifact in place — both are stored,
        // and each client keeps getting its own.
        build_wanted_artifact(&cache, &wanted_by(&gzip_hit));
        assert_eq!(
            serve_as(&file_path, &dir, &cache, Coding::Gzip)
                .await
                .headers()[header::CONTENT_ENCODING],
            "gzip"
        );
        assert_eq!(
            serve_br(&file_path, &dir, &cache).await.headers()[header::CONTENT_ENCODING],
            "br"
        );
    }

    #[tokio::test]
    async fn test_no_artifact_path_without_brotli_support() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("app.css");
        fs::write(&file_path, css_body()).unwrap();
        let cache = Arc::new(FileCache::new(10));

        // Warm the content cache, then hit it as a client that does not accept
        // Brotli: no ask, and no encoded body even once an artifact exists.
        serve_with(&file_path, &dir, &cache, HeaderMap::new()).await;
        let cached = serve_with(&file_path, &dir, &cache, HeaderMap::new()).await;
        assert!(cached.extensions().get::<ArtifactWanted>().is_none());

        let key = file_path.to_string_lossy().to_string();
        let modified = fs::metadata(&file_path).unwrap().modified().unwrap();
        let artifact =
            crate::server::compression::compress_artifact(css_body().as_bytes(), Coding::Br)
                .unwrap();
        cache.insert_artifact(&key, Coding::Br, modified, Bytes::from(artifact));

        let identity = serve_with(&file_path, &dir, &cache, HeaderMap::new()).await;
        assert!(identity.headers().get(header::CONTENT_ENCODING).is_none());
        assert_eq!(body_bytes(identity).await.len(), css_body().len());
    }

    #[tokio::test]
    async fn test_no_artifact_for_incompressible_type() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("photo.png");
        fs::write(&file_path, vec![0u8; 4096]).unwrap();
        let cache = Arc::new(FileCache::new(10));

        serve_br(&file_path, &dir, &cache).await;
        let cached = serve_br(&file_path, &dir, &cache).await;
        // image/png is outside the compressible list, so ranges stay on and
        // nothing asks for an artifact that would never be served.
        assert!(cached.extensions().get::<ArtifactWanted>().is_none());
        assert_eq!(cached.headers()[header::ACCEPT_RANGES], "bytes");
    }

    #[tokio::test]
    async fn test_incompressible_entry_stops_asking() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("noise.css");
        // Compressible MIME type, incompressible bytes: a deterministic LCG
        // stream that Brotli cannot shrink below its own input.
        let mut state = 0x2545_f491u32;
        let noise: Vec<u8> = (0..2048)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (state >> 24) as u8
            })
            .collect();
        fs::write(&file_path, &noise).unwrap();
        let cache = Arc::new(FileCache::new(10));
        let key = file_path.to_string_lossy().to_string();

        serve_br(&file_path, &dir, &cache).await;
        let wanted = serve_br(&file_path, &dir, &cache)
            .await
            .extensions()
            .get::<ArtifactWanted>()
            .expect("a compressible MIME type asks before it knows the bytes resist")
            .clone();
        assert!(
            crate::server::compression::compress_artifact(&wanted.bytes, wanted.coding).is_none(),
            "the fixture must not compress, or this test proves nothing"
        );

        // What the background job does when compression does not pay off.
        cache.reject_artifact(&wanted.key, wanted.coding, wanted.modified);

        let after = serve_br(&file_path, &dir, &cache).await;
        assert!(
            after.extensions().get::<ArtifactWanted>().is_none(),
            "a rejected entry must not keep scheduling a compression that cannot help"
        );
        assert!(after.headers().get(header::CONTENT_ENCODING).is_none());
        assert_eq!(cache.content.read().total_bytes, noise.len());
        assert!(key.ends_with("noise.css"));
    }

    #[test]
    fn test_artifact_claim_is_single_flight() {
        let cache = Arc::new(FileCache::new(10));
        let claim = cache
            .claim_artifact("/w/app.css", Coding::Br)
            .expect("first claim wins");
        assert_eq!(claim.key(), "/w/app.css");
        assert!(
            cache.claim_artifact("/w/app.css", Coding::Br).is_none(),
            "a second claim on the same key must not start a second compression"
        );
        // A different file is unaffected.
        assert!(cache.claim_artifact("/w/other.css", Coding::Br).is_some());
        drop(claim);
        assert!(
            cache.claim_artifact("/w/app.css", Coding::Br).is_some(),
            "the claim must be released when the job's guard drops"
        );
    }

    #[test]
    fn test_stale_artifact_is_dropped() {
        let cache = Arc::new(FileCache::new(10));
        let key = "/w/app.css".to_string();
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        cache.insert_content(
            key.clone(),
            Bytes::from_static(b"identity"),
            "text/css".into(),
            modified,
            "\"tag\"".into(),
            "date".into(),
        );

        // The file changed while the job ran: the artifact describes bytes the
        // entry no longer holds.
        cache.insert_artifact(
            &key,
            Coding::Br,
            modified + Duration::from_secs(1),
            Bytes::from_static(b"x"),
        );
        assert!(matches!(
            cache.content.read().entries.peek(&key).unwrap().artifacts[Coding::Br.slot()],
            ArtifactState::Absent
        ));

        // Matching mtime is accepted, and charged to the budget.
        let before = cache.content.read().total_bytes;
        cache.insert_artifact(&key, Coding::Br, modified, Bytes::from_static(b"xy"));
        let guard = cache.content.read();
        assert!(matches!(
            &guard.entries.peek(&key).unwrap().artifacts[Coding::Br.slot()],
            ArtifactState::Ready(bytes) if &bytes[..] == b"xy"
        ));
        assert_eq!(guard.total_bytes, before + 2);
    }

    #[test]
    fn test_a_stale_rejection_does_not_condemn_the_new_bytes() {
        let cache = Arc::new(FileCache::new(10));
        let key = "/w/app.css".to_string();
        let modified = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        cache.insert_content(
            key.clone(),
            Bytes::from_static(b"identity"),
            "text/css".into(),
            modified,
            "\"tag\"".into(),
            "date".into(),
        );

        // The job that found the bytes incompressible was working on the file
        // as it was before it changed. Recording that verdict against the entry
        // now would mark bytes it never saw as hopeless, permanently.
        cache.reject_artifact(&key, Coding::Br, modified - Duration::from_secs(1));
        assert!(matches!(
            cache.content.read().entries.peek(&key).unwrap().artifacts[Coding::Br.slot()],
            ArtifactState::Absent
        ));

        // A verdict on the bytes the entry actually holds is recorded.
        cache.reject_artifact(&key, Coding::Br, modified);
        assert!(matches!(
            cache.content.read().entries.peek(&key).unwrap().artifacts[Coding::Br.slot()],
            ArtifactState::Rejected
        ));
    }

    #[test]
    fn test_artifact_is_not_rebuilt_over_an_existing_one() {
        let cache = Arc::new(FileCache::new(10));
        let key = "/w/app.css".to_string();
        let modified = SystemTime::UNIX_EPOCH;
        cache.insert_content(
            key.clone(),
            Bytes::from_static(b"identity"),
            "text/css".into(),
            modified,
            "\"tag\"".into(),
            "date".into(),
        );
        cache.insert_artifact(&key, Coding::Br, modified, Bytes::from_static(b"first"));
        let charged = cache.content.read().total_bytes;
        cache.insert_artifact(&key, Coding::Br, modified, Bytes::from_static(b"second"));
        let guard = cache.content.read();
        assert!(
            matches!(
                &guard.entries.peek(&key).unwrap().artifacts[Coding::Br.slot()],
                ArtifactState::Ready(bytes) if &bytes[..] == b"first"
            ),
            "a late second job must not overwrite a stored artifact"
        );
        assert_eq!(guard.total_bytes, charged, "nor charge the budget twice");
    }

    async fn serve_with(
        file_path: &std::path::Path,
        dir: &TempDir,
        cache: &FileCache,
        headers: HeaderMap,
    ) -> Response<ResponseBody> {
        serve_at(file_path, dir, cache, headers, offered()).await
    }

    /// Serve as an identity client against a given compression configuration.
    async fn serve_at(
        file_path: &std::path::Path,
        dir: &TempDir,
        cache: &FileCache,
        headers: HeaderMap,
        levels: Levels,
    ) -> Response<ResponseBody> {
        serve(
            file_path,
            cache,
            &canonical_root(dir),
            &empty_allow(),
            &http::Method::GET,
            &headers,
            Some("public, max-age=86400"),
            None,
            levels,
        )
        .await
        .unwrap()
    }

    async fn body_bytes(response: Response<ResponseBody>) -> Bytes {
        use http_body_util::BodyExt;
        response.into_body().collect().await.unwrap().to_bytes()
    }

    fn range_headers(range: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, range.parse().unwrap());
        headers
    }

    #[test]
    fn test_etag_for_304_echoes_client_weakness() {
        let modified = test_modified();
        let etag = generate_etag(100, &modified);

        // Client revalidates a compressed copy with the weakened tag —
        // the 304 must echo the weak form, not re-strengthen it.
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, format!("W/{etag}").parse().unwrap());
        assert_eq!(etag_for_304(&headers, &etag), format!("W/{etag}"));

        // Client with an identity copy keeps the strong tag.
        headers.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        assert_eq!(etag_for_304(&headers, &etag), etag);

        // No If-None-Match (date-based 304): strong tag.
        assert_eq!(etag_for_304(&HeaderMap::new(), &etag), etag);
    }

    #[tokio::test]
    async fn test_serve_304_weak_inm_echoes_weak_etag() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "hello").unwrap();

        let cache = FileCache::new(10);
        let response = serve_with(&file_path, &dir, &cache, HeaderMap::new()).await;
        let etag = response
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, format!("W/{etag}").parse().unwrap());
        let response = serve_with(&file_path, &dir, &cache, headers).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
        assert_eq!(
            response
                .headers()
                .get(header::ETAG)
                .unwrap()
                .to_str()
                .unwrap(),
            format!("W/{etag}")
        );
    }

    #[tokio::test]
    async fn test_serve_accept_ranges_on_200() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("plain.txt");
        fs::write(&file_path, "0123456789").unwrap();

        let cache = FileCache::new(10);
        let response = serve_with(&file_path, &dir, &cache, HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::ACCEPT_RANGES).unwrap(),
            "bytes"
        );
    }

    #[tokio::test]
    async fn test_serve_206_small_file_and_cached() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "0123456789").unwrap();

        let cache = FileCache::new(10);

        // First request: small-file read path (also populates the cache)
        let response = serve_with(&file_path, &dir, &cache, range_headers("bytes=2-5")).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 2-5/10"
        );
        assert_eq!(response.headers().get(header::CONTENT_LENGTH).unwrap(), "4");
        assert_eq!(
            response.headers().get(header::ACCEPT_RANGES).unwrap(),
            "bytes"
        );
        assert_eq!(body_bytes(response).await, &b"2345"[..]);

        // Second request: content-cache hit path
        let response = serve_with(&file_path, &dir, &cache, range_headers("bytes=-3")).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 7-9/10"
        );
        assert_eq!(body_bytes(response).await, &b"789"[..]);
    }

    #[tokio::test]
    async fn test_serve_206_streamed_large_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("large.bin");
        // > MAX_CACHE_FILE_SIZE so the streaming path is taken
        let mut data = vec![0u8; MAX_CACHE_FILE_SIZE + 100];
        for (i, b) in data.iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        fs::write(&file_path, &data).unwrap();

        let cache = FileCache::new(10);
        let start = MAX_CACHE_FILE_SIZE as u64 + 10;
        let end = start + 49;
        let response = serve_with(
            &file_path,
            &dir,
            &cache,
            range_headers(&format!("bytes={start}-{end}")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_RANGE)
                .unwrap()
                .to_str()
                .unwrap(),
            format!("bytes {start}-{end}/{}", data.len())
        );
        assert_eq!(
            response.headers().get(header::CONTENT_LENGTH).unwrap(),
            "50"
        );
        let body = body_bytes(response).await;
        assert_eq!(&body[..], &data[start as usize..=end as usize]);
    }

    #[tokio::test]
    async fn test_serve_416_unsatisfiable() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "0123456789").unwrap();

        let cache = FileCache::new(10);
        let response = serve_with(&file_path, &dir, &cache, range_headers("bytes=100-")).await;
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            response.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes */10"
        );
    }

    #[tokio::test]
    async fn test_serve_416_streamed_large_file() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("large.bin");
        let size = MAX_CACHE_FILE_SIZE + 1;
        fs::write(&file_path, vec![0u8; size]).unwrap();

        let cache = FileCache::new(10);
        let response = serve_with(
            &file_path,
            &dir,
            &cache,
            range_headers(&format!("bytes={size}-")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            response
                .headers()
                .get(header::CONTENT_RANGE)
                .unwrap()
                .to_str()
                .unwrap(),
            format!("bytes */{size}")
        );
    }

    #[tokio::test]
    async fn test_serve_if_range_mismatch_returns_full() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "0123456789").unwrap();

        let cache = FileCache::new(10);
        let mut headers = range_headers("bytes=0-4");
        headers.insert(header::IF_RANGE, "\"stale-etag\"".parse().unwrap());
        let response = serve_with(&file_path, &dir, &cache, headers).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_bytes(response).await, &b"0123456789"[..]);
    }

    #[tokio::test]
    async fn test_serve_if_range_match_returns_partial() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "0123456789").unwrap();

        let cache = FileCache::new(10);

        // Fetch the current ETag first
        let response = serve_with(&file_path, &dir, &cache, HeaderMap::new()).await;
        let etag = response
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let mut headers = range_headers("bytes=0-4");
        headers.insert(header::IF_RANGE, etag.parse().unwrap());
        let response = serve_with(&file_path, &dir, &cache, headers).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(body_bytes(response).await, &b"01234"[..]);
    }

    #[tokio::test]
    async fn test_serve_multi_range_falls_back_to_full() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "0123456789").unwrap();

        let cache = FileCache::new(10);
        let response = serve_with(&file_path, &dir, &cache, range_headers("bytes=0-1,4-5")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_bytes(response).await, &b"0123456789"[..]);
    }

    #[tokio::test]
    async fn test_serve_head_range_returns_206_headers() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "0123456789").unwrap();

        let cache = FileCache::new(10);
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::HEAD,
            &range_headers("bytes=0-0"),
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
        // The body is elided on the wire by hyper; the headers must match GET's.
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 0-0/10"
        );
    }

    #[tokio::test]
    async fn test_serve_range_ignored_for_post() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "0123456789").unwrap();

        let cache = FileCache::new(10);
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::POST,
            &range_headers("bytes=0-4"),
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_bytes(response).await, &b"0123456789"[..]);
    }

    #[tokio::test]
    async fn test_serve_post_conditional_not_304() {
        // RFC 9110 §13.1.2-3: 304 is a GET/HEAD response — a POST carrying a
        // matching If-None-Match still gets the full 200.
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "hello").unwrap();

        let cache = FileCache::new(10);
        let response = serve_with(&file_path, &dir, &cache, HeaderMap::new()).await;
        let etag = response
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::POST,
            &headers,
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body_bytes(response).await, &b"hello"[..]);
    }

    #[tokio::test]
    async fn test_serve_range_on_empty_file_returns_200() {
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("empty.txt");
        fs::write(&file_path, "").unwrap();

        let cache = FileCache::new(10);
        let response = serve_with(&file_path, &dir, &cache, range_headers("bytes=0-")).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers().get(header::CONTENT_LENGTH).unwrap(), "0");
        assert!(body_bytes(response).await.is_empty());
    }

    #[tokio::test]
    async fn test_serve_no_range_for_compressible_when_brotli_accepted() {
        // A brotli-accepting client may hold a compressed copy of this
        // representation; identity 206 fragments would corrupt it (and no
        // If-Range form can tell the copies apart). Ranges are disabled for
        // would-be-compressed responses, like nginx's gzip filter.
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("app.css");
        let body = "x".repeat(500); // compressible type, within compress window
        fs::write(&file_path, &body).unwrap();

        let cache = FileCache::new(10);
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &range_headers("bytes=0-9"),
            Some("public, max-age=86400"),
            Some(Coding::Br),
            offered(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        // The header must not advertise ranges the server just ignored —
        // even if brotli later declines to encode (output not smaller),
        // leaving the response uncompressed.
        assert!(response.headers().get(header::ACCEPT_RANGES).is_none());
        assert_eq!(body_bytes(response).await.len(), 500);

        // Same request without brotli support gets the range.
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &range_headers("bytes=0-9"),
            Some("public, max-age=86400"),
            None,
            offered(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    }

    #[tokio::test]
    async fn test_serve_range_for_non_compressible_with_brotli() {
        // Non-compressible types are never encoded — ranges stay enabled
        // even for brotli-accepting clients (the case that matters: video,
        // archives, images).
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.bin");
        fs::write(&file_path, vec![7u8; 500]).unwrap();

        let cache = FileCache::new(10);
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &range_headers("bytes=0-9"),
            Some("public, max-age=86400"),
            Some(Coding::Br),
            offered(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(header::CONTENT_RANGE).unwrap(),
            "bytes 0-9/500"
        );
        // Never compressed for any client — nothing varies by encoding.
        assert!(response.headers().get(header::VARY).is_none());
    }

    /// One byte past the top of the compression window, so the file streams
    /// however compressible it looks and whatever the client accepts.
    const ABOVE_COMPRESSION_WINDOW: usize = 3 * 1024 * 1024 + 1;

    #[tokio::test]
    async fn test_serve_range_for_streamed_compressible_with_brotli() {
        // A file too large to compress streams from disk, so no encoded
        // representation of it exists anywhere — disabling ranges there would
        // break resumption for zero benefit.
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("app.js");
        let size = ABOVE_COMPRESSION_WINDOW;
        fs::write(&file_path, vec![b'x'; size]).unwrap();

        let cache = FileCache::new(10);
        let response = serve(
            &file_path,
            &cache,
            &canonical_root(&dir),
            &empty_allow(),
            &http::Method::GET,
            &range_headers("bytes=0-9"),
            Some("public, max-age=86400"),
            Some(Coding::Br), // irrelevant on the streaming path
            offered(),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(header::CONTENT_RANGE).unwrap(),
            format!("bytes 0-9/{size}").as_str()
        );
        assert_eq!(
            response.headers().get(header::ACCEPT_RANGES).unwrap(),
            "bytes"
        );
        // Streamed responses are identity for every client — no Vary.
        assert!(response.headers().get(header::VARY).is_none());
        assert_eq!(body_bytes(response).await.len(), 10);
    }

    /// A file between the cache limit and the top of the compression window.
    /// Large enough that streaming it is the obvious thing to do, and exactly
    /// the size where doing so costs the client megabytes.
    fn band_css(dir: &TempDir, name: &str) -> std::path::PathBuf {
        let file_path = dir.path().join(name);
        let body = css_body().repeat(800);
        assert!(body.len() > MAX_CACHE_FILE_SIZE && body.len() < ABOVE_COMPRESSION_WINDOW);
        fs::write(&file_path, &body).unwrap();
        file_path
    }

    #[tokio::test]
    async fn test_file_above_the_cache_limit_is_read_whole_for_a_coding_client() {
        // The compression layer encodes a body it holds whole and passes a
        // stream through, so serving this one as a stream is the same as
        // serving it uncompressed. It is read instead — and the exact size
        // hint is the property the compression layer actually keys on.
        let dir = TempDir::new().unwrap();
        let file_path = band_css(&dir, "app.css");
        let size = fs::metadata(&file_path).unwrap().len();

        let cache = FileCache::new(10);
        let response = serve_br(&file_path, &dir, &cache).await;

        assert_eq!(
            hyper::body::Body::size_hint(response.body()).exact(),
            Some(size)
        );
        // Ranges are off for the same reason they are off for a cached file
        // this client would get encoded: identity fragments cannot be spliced
        // onto an encoded prefix.
        assert!(response.headers().get(header::ACCEPT_RANGES).is_none());
        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            "Accept-Encoding"
        );
        // Read for this request only: putting it in the cache would spend the
        // budget meant for small files on one that does not fit it.
        assert!(cache
            .lookup(
                &file_path.to_string_lossy(),
                &HeaderMap::new(),
                false,
                Some(Coding::Br)
            )
            .is_none());
    }

    #[tokio::test]
    async fn test_the_same_file_still_streams_for_an_identity_client() {
        // Nothing would encode this body, so reading it whole would buy the
        // client nothing and cost the server the whole file in memory.
        let dir = TempDir::new().unwrap();
        let file_path = band_css(&dir, "app.css");

        let cache = FileCache::new(10);
        let response = serve_with(&file_path, &dir, &cache, HeaderMap::new()).await;

        assert_eq!(hyper::body::Body::size_hint(response.body()).exact(), None);
        assert_eq!(
            response.headers().get(header::ACCEPT_RANGES).unwrap(),
            "bytes"
        );
        // The other client got a quarter of these bytes off the same URL. A
        // shared cache that stored this response unkeyed would hand the full
        // megabyte to that client from then on.
        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            "Accept-Encoding"
        );
    }

    #[tokio::test]
    async fn test_an_unsatisfiable_range_on_a_streamed_band_file_varies() {
        // A client that negotiated a coding gets ranges disabled and a 200 for
        // this request, so even the refusal depends on Accept-Encoding.
        let dir = TempDir::new().unwrap();
        let file_path = band_css(&dir, "app.css");
        let size = fs::metadata(&file_path).unwrap().len();

        let cache = FileCache::new(10);
        let response = serve_with(
            &file_path,
            &dir,
            &cache,
            range_headers(&format!("bytes={size}-")),
        )
        .await;

        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            "Accept-Encoding"
        );
    }

    #[tokio::test]
    async fn test_a_file_past_the_compression_window_does_not_vary() {
        // Nothing this server offers would ever encode it, so its bytes are
        // the same for every client and a cache may key on the URL alone.
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("huge.css");
        fs::write(&file_path, vec![b'x'; ABOVE_COMPRESSION_WINDOW]).unwrap();

        let cache = FileCache::new(10);
        let response = serve_with(&file_path, &dir, &cache, HeaderMap::new()).await;

        assert_eq!(hyper::body::Body::size_hint(response.body()).exact(), None);
        assert!(response.headers().get(header::VARY).is_none());
    }

    #[tokio::test]
    async fn test_a_band_file_streams_when_no_permit_is_free() {
        // The read is what makes this file compressible, and it is capped: the
        // bytes stay resident until the response is written, and the
        // connection limit is far too generous a ceiling for that. Over the
        // cap the file streams uncompressed, which is what it was before this
        // path existed — and it still varies, because the request that took
        // the last permit was answered compressed.
        let dir = TempDir::new().unwrap();
        let file_path = band_css(&dir, "app.css");

        let cache = FileCache::new(10);
        let held: Vec<_> =
            std::iter::repeat_with(|| Arc::clone(&cache.off_disk_compression).try_acquire_owned())
                .take(MAX_OFF_DISK_COMPRESSIONS)
                .collect::<Result<_, _>>()
                .expect("a fresh cache hands out its whole cap");

        let response = serve_br(&file_path, &dir, &cache).await;
        assert_eq!(hyper::body::Body::size_hint(response.body()).exact(), None);
        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            "Accept-Encoding"
        );

        // Releasing them puts the file back on the buffered path.
        drop(held);
        let response = serve_br(&file_path, &dir, &cache).await;
        assert!(hyper::body::Body::size_hint(response.body())
            .exact()
            .is_some());
    }

    #[tokio::test]
    async fn test_a_permit_is_held_until_the_response_is_dropped() {
        // Releasing it when `serve` returns would count nothing: the bytes are
        // still resident, and the compression that justifies the cap has not
        // run yet.
        let dir = TempDir::new().unwrap();
        let file_path = band_css(&dir, "app.css");

        let cache = FileCache::new(10);
        let response = serve_br(&file_path, &dir, &cache).await;
        assert_eq!(
            cache.off_disk_compression.available_permits(),
            MAX_OFF_DISK_COMPRESSIONS - 1
        );

        drop(response);
        assert_eq!(
            cache.off_disk_compression.available_permits(),
            MAX_OFF_DISK_COMPRESSIONS
        );
    }

    #[tokio::test]
    async fn test_nothing_varies_when_no_coding_is_offered() {
        // `COMPRESSION_ENCODINGS=off`: the server never reads Accept-Encoding,
        // so declaring that the response depends on it only fragments a
        // downstream cache over a header that changes nothing.
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("app.css");
        fs::write(&file_path, "x".repeat(500)).unwrap();

        let cache = FileCache::new(10);
        let response = serve_at(&file_path, &dir, &cache, HeaderMap::new(), no_codings()).await;

        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get(header::VARY).is_none());
        // And ranges are never withheld, since nothing is ever encoded.
        assert_eq!(
            response.headers().get(header::ACCEPT_RANGES).unwrap(),
            "bytes"
        );
    }

    #[tokio::test]
    async fn test_an_incompressible_type_in_the_band_still_streams() {
        // Size alone does not decide it: these bytes carry their own
        // compression, so the coding the client accepts is never applied.
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("photo.png");
        fs::write(&file_path, vec![b'x'; MAX_CACHE_FILE_SIZE + 4096]).unwrap();

        let cache = FileCache::new(10);
        let response = serve_br(&file_path, &dir, &cache).await;

        assert_eq!(hyper::body::Body::size_hint(response.body()).exact(), None);
        assert_eq!(
            response.headers().get(header::ACCEPT_RANGES).unwrap(),
            "bytes"
        );
    }

    #[tokio::test]
    async fn test_serve_vary_for_compression_eligible_identity() {
        // For a representation in the compression window the response form
        // (encoding, 200 vs 206 vs 416, Accept-Ranges) depends on
        // Accept-Encoding — identity responses must carry Vary too, or a
        // shared cache would serve them to brotli clients unkeyed.
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("app.css");
        fs::write(&file_path, "x".repeat(500)).unwrap();

        let cache = FileCache::new(10);
        // serve_with passes supports_brotli = false → identity response
        let response = serve_with(&file_path, &dir, &cache, HeaderMap::new()).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            "Accept-Encoding"
        );

        let response = serve_with(&file_path, &dir, &cache, range_headers("bytes=0-9")).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            "Accept-Encoding"
        );

        // A brotli client would have gotten 200 here — the 416 varies too.
        let response = serve_with(&file_path, &dir, &cache, range_headers("bytes=999-")).await;
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(
            response.headers().get(header::VARY).unwrap(),
            "Accept-Encoding"
        );
    }

    #[tokio::test]
    async fn test_serve_304_takes_precedence_over_range() {
        // RFC 9110 §13.2.2: If-None-Match is evaluated before Range
        let dir = TempDir::new().unwrap();
        let file_path = dir.path().join("data.txt");
        fs::write(&file_path, "0123456789").unwrap();

        let cache = FileCache::new(10);
        let response = serve_with(&file_path, &dir, &cache, HeaderMap::new()).await;
        let etag = response
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_string();

        let mut headers = range_headers("bytes=0-4");
        headers.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        let response = serve_with(&file_path, &dir, &cache, headers).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    }
}
