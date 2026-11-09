//! Disk writer for profile runs. Fans out one file per configured
//! format plus a per-line `index.json` entry. Rate-limited via token bucket;
//! drops with WARN when the bucket is empty.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;

use tokio::fs::{self, OpenOptions};
use tokio::io::AsyncWriteExt;

use crate::profiling::export::{export_collapsed, export_xhprof};
use crate::profiling::SpanTree;

use super::RunMeta;

/// An exporter the disk writer can fan out to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Xhprof,
    Collapsed,
}

impl OutputFormat {
    /// Parse a config-string token into a format. Accepts the
    /// short name (`xhprof`) or the full extension (`xhprof.json`).
    pub fn from_str_opt(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "xhprof" | "xhprof.json" => Some(Self::Xhprof),
            "collapsed" => Some(Self::Collapsed),
            _ => None,
        }
    }

    /// File extension (after the run_id).
    pub fn extension(self) -> &'static str {
        match self {
            Self::Xhprof => "xhprof.json",
            Self::Collapsed => "collapsed",
        }
    }

    /// Serialize `tree` in this format.
    pub fn render(self, tree: &SpanTree) -> Vec<u8> {
        match self {
            Self::Xhprof => export_xhprof(tree),
            Self::Collapsed => export_collapsed(tree),
        }
    }
}

/// Splits configured names into the formats they select and the names no
/// format answers to, which the caller reports. A list with no known name
/// selects the defaults, so that naming only formats this version lacks does
/// not turn disk output off. An empty list stays empty: that is how disk
/// output is turned off.
pub fn parse_output_formats(tokens: &[String]) -> (Vec<OutputFormat>, Vec<String>) {
    let mut known = Vec::with_capacity(tokens.len());
    let mut unknown = Vec::new();
    for token in tokens {
        match OutputFormat::from_str_opt(token) {
            Some(format) => known.push(format),
            None => unknown.push(token.clone()),
        }
    }
    if known.is_empty() && !unknown.is_empty() {
        known = vec![OutputFormat::Xhprof, OutputFormat::Collapsed];
    }
    (known, unknown)
}

/// Token bucket rate limiter — `try_consume` refills since
/// `last_refill` at `rate_per_sec`, then attempts to subtract.
pub struct TokenBucket {
    tokens: f64,
    capacity: f64,
    rate_per_sec: f64,
    last_refill: Instant,
}

impl TokenBucket {
    pub fn new(rate_per_sec: f64, capacity: f64) -> Self {
        Self {
            tokens: capacity,
            capacity,
            rate_per_sec,
            last_refill: Instant::now(),
        }
    }

    pub fn try_consume(&mut self, n: f64) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.last_refill = now;
        self.tokens = (self.tokens + elapsed * self.rate_per_sec).min(self.capacity);
        if self.tokens >= n {
            self.tokens -= n;
            true
        } else {
            false
        }
    }
}

/// Validate a `run_id` against path-traversal: only ASCII
/// alphanumerics, `_`, and `-`; non-empty; ≤128 chars.
pub fn run_id_is_safe(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 128
        && run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

pub struct DiskWriter {
    pub output_dir: PathBuf,
    bucket: Mutex<TokenBucket>,
    metrics: Arc<super::StorageMetrics>,
    /// Serialises every read-modify-write of `index.json`. Shared
    /// with `RetentionTask` (async sweep) so append and rewrite are
    /// totally ordered regardless of which one starts first.
    /// Without this, retention's rewrite races with append, losing
    /// entries on Linux and crashing on Windows.
    pub index_lock: Arc<tokio::sync::Mutex<()>>,
}

impl DiskWriter {
    pub fn new(
        output_dir: PathBuf,
        rate_per_sec: f64,
        capacity: f64,
        metrics: Arc<super::StorageMetrics>,
    ) -> Self {
        Self::with_index_lock(
            output_dir,
            rate_per_sec,
            capacity,
            metrics,
            Arc::new(tokio::sync::Mutex::new(())),
        )
    }

    /// Construct a `DiskWriter` sharing `index_lock` with a
    /// `RetentionTask`. The plugin init site uses this so both
    /// writers coordinate on the same mutex; tests and standalone
    /// use-cases use `new()` which mints a fresh lock.
    pub fn with_index_lock(
        output_dir: PathBuf,
        rate_per_sec: f64,
        capacity: f64,
        metrics: Arc<super::StorageMetrics>,
        index_lock: Arc<tokio::sync::Mutex<()>>,
    ) -> Self {
        Self {
            output_dir,
            bucket: Mutex::new(TokenBucket::new(rate_per_sec, capacity)),
            metrics,
            index_lock,
        }
    }

    /// Fan out the chosen formats + append the index entry.
    /// Returns `false` and drops the run silently when:
    /// - `run_id` fails the path-traversal validator.
    /// - The token bucket has no tokens (rate-limited).
    /// - The output directory cannot be created.
    pub async fn write_run(
        &self,
        meta: &RunMeta,
        tree: &Arc<SpanTree>,
        formats: &[OutputFormat],
    ) -> bool {
        if !run_id_is_safe(&meta.run_id) {
            tracing::error!(
                plugin = "profiler",
                run_id = %meta.run_id,
                "rejecting unsafe run_id"
            );
            return false;
        }
        if !self.bucket.lock().try_consume(1.0) {
            tracing::warn!(
                plugin = "profiler",
                run_id = %meta.run_id,
                "disk write rate-limited; dropping run"
            );
            self.metrics.inc_disk_drop();
            return false;
        }
        if let Err(e) = fs::create_dir_all(&self.output_dir).await {
            tracing::error!(
                plugin = "profiler",
                output_dir = %self.output_dir.display(),
                error = %e,
                "failed to create output dir; dropping run"
            );
            self.metrics.inc_disk_drop();
            return false;
        }

        // Fan out the format writes via tokio::spawn so they happen
        // in parallel. Each future returns `Some(format_extension)`
        // on success; collect those for the index entry.
        let mut tasks = Vec::with_capacity(formats.len());
        for &fmt in formats {
            let bytes = fmt.render(tree);
            let path = self
                .output_dir
                .join(format!("{}.{}", meta.run_id, fmt.extension()));
            let fmt_name = fmt.extension().to_string();
            let metrics = Arc::clone(&self.metrics);
            let fmt_copy = fmt;
            let bytes_len = bytes.len() as u64;
            tasks.push(tokio::spawn(async move {
                match fs::write(&path, &bytes).await {
                    Ok(()) => {
                        metrics.add_bytes(fmt_copy, bytes_len);
                        Some(fmt_name)
                    }
                    Err(e) => {
                        tracing::error!(
                            plugin = "profiler",
                            path = %path.display(),
                            error = %e,
                            "format write failed"
                        );
                        metrics.inc_disk_drop();
                        None
                    }
                }
            }));
        }
        let mut written: Vec<String> = Vec::with_capacity(formats.len());
        for t in tasks {
            if let Ok(Some(name)) = t.await {
                written.push(name);
            }
        }
        // Index entry — append-only NDJSON, one object per line.
        let mut entry = meta.clone();
        entry.formats = written;
        if let Err(e) = append_index_entry(&self.output_dir, &entry, &self.index_lock).await {
            tracing::error!(
                plugin = "profiler",
                run_id = %meta.run_id,
                error = %e,
                "index.json append failed (data files written)"
            );
        }
        true
    }
}

async fn append_index_entry(
    output_dir: &Path,
    meta: &RunMeta,
    index_lock: &tokio::sync::Mutex<()>,
) -> std::io::Result<()> {
    let mut line = serde_json::to_vec(meta).expect("serialise RunMeta");
    line.push(b'\n');
    let path = output_dir.join("index.json");
    // Hold the shared index.json mutex across the open+write+flush.
    // Even though `O_APPEND` writes are atomic on Linux for small
    // writes, we must still serialise with retention's rewrite-via-rename
    // — otherwise we could open the
    // pre-rename fd and write into a file that is about to be
    // unlinked, losing the appended entry.
    let _guard = index_lock.lock().await;
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .await?;
    f.write_all(&line).await?;
    f.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_bucket_starts_full() {
        let mut b = TokenBucket::new(10.0, 5.0);
        for _ in 0..5 {
            assert!(b.try_consume(1.0));
        }
        assert!(!b.try_consume(1.0));
    }

    #[test]
    fn token_bucket_refills_after_sleep() {
        let mut b = TokenBucket::new(100.0, 1.0);
        assert!(b.try_consume(1.0));
        assert!(!b.try_consume(1.0));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(b.try_consume(1.0), "should have refilled by now");
    }

    #[test]
    fn run_id_validator_rejects_unsafe_paths() {
        assert!(run_id_is_safe("abc-123_DEF"));
        assert!(!run_id_is_safe(""));
        assert!(!run_id_is_safe("../etc/passwd"));
        assert!(!run_id_is_safe("a/b"));
        assert!(!run_id_is_safe("a.b"));
    }

    #[test]
    fn xhprof_answers_to_its_file_extension_too() {
        for name in ["xhprof", "xhprof.json"] {
            assert_eq!(
                OutputFormat::from_str_opt(name),
                Some(OutputFormat::Xhprof),
                "{name}"
            );
        }
    }

    #[test]
    fn removed_formats_do_not_parse() {
        for name in ["pprof", "speedscope", "speedscope.json"] {
            assert_eq!(OutputFormat::from_str_opt(name), None, "{name}");
        }
    }

    #[test]
    fn parse_output_formats_reports_unknown_names() {
        let tokens = [
            "xhprof".to_string(),
            "pprof".to_string(),
            "collapsed".to_string(),
        ];
        let (known, unknown) = parse_output_formats(&tokens);
        assert_eq!(known, vec![OutputFormat::Xhprof, OutputFormat::Collapsed]);
        assert_eq!(unknown, vec!["pprof".to_string()]);
    }

    #[test]
    fn a_list_with_no_known_name_selects_the_default_formats() {
        let tokens = ["speedscope".to_string(), "pprof".to_string()];
        let (known, unknown) = parse_output_formats(&tokens);
        assert_eq!(known, vec![OutputFormat::Xhprof, OutputFormat::Collapsed]);
        assert_eq!(unknown, tokens.to_vec());
    }

    #[test]
    fn an_empty_list_selects_no_format() {
        let (known, unknown) = parse_output_formats(&[]);
        assert!(known.is_empty());
        assert!(unknown.is_empty());
    }

    #[test]
    fn output_format_string_round_trip() {
        for fmt in [OutputFormat::Xhprof, OutputFormat::Collapsed] {
            let s = fmt.extension();
            assert!(OutputFormat::from_str_opt(s).is_some(), "{s}");
        }
    }
}
