//! Prometheus counter/gauge state for the profiler storage pipeline.
//!
//! Hung off `Storage` as a single `Arc<StorageMetrics>`. Incremented
//! by the disk writer, HTTP pusher, and `ProfilerCompleteHandler`;
//! collected in `ox_profiler::routes` via `ctx.register_metrics`.

use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use super::disk::OutputFormat;
use crate::plugins::ox_profiler::trigger::ActivationSource;

const FORMAT_COUNT: usize = 4;
const SOURCE_COUNT: usize = 4;

pub struct StorageMetrics {
    pub runs_total: [AtomicU64; SOURCE_COUNT],
    pub spans_collected_total: AtomicU64,
    pub bytes_written_total: [AtomicU64; FORMAT_COUNT],
    pub disk_drops_total: AtomicU64,
    pub http_push_failures_total: AtomicU64,
    pub truncated_total: AtomicU64,
    /// Runs dropped at the spawn fan-out because the disk admission
    /// semaphore was saturated (backend slow/unavailable).
    pub disk_saturated_drops_total: AtomicU64,
    /// Runs dropped at the spawn fan-out because the HTTP admission
    /// semaphore was saturated (backend slow/unavailable).
    pub http_saturated_drops_total: AtomicU64,
}

impl Default for StorageMetrics {
    fn default() -> Self {
        Self {
            runs_total: Default::default(),
            spans_collected_total: AtomicU64::new(0),
            bytes_written_total: Default::default(),
            disk_drops_total: AtomicU64::new(0),
            http_push_failures_total: AtomicU64::new(0),
            truncated_total: AtomicU64::new(0),
            disk_saturated_drops_total: AtomicU64::new(0),
            http_saturated_drops_total: AtomicU64::new(0),
        }
    }
}

impl StorageMetrics {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn inc_runs(&self, source: ActivationSource) {
        self.runs_total[source_index(source)].fetch_add(1, Ordering::Relaxed);
    }
    pub fn add_spans(&self, n: u64) {
        self.spans_collected_total.fetch_add(n, Ordering::Relaxed);
    }
    pub fn add_bytes(&self, fmt: OutputFormat, n: u64) {
        self.bytes_written_total[format_index(fmt)].fetch_add(n, Ordering::Relaxed);
    }
    pub fn inc_disk_drop(&self) {
        self.disk_drops_total.fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_http_push_failure(&self) {
        self.http_push_failures_total
            .fetch_add(1, Ordering::Relaxed);
    }
    pub fn inc_truncated(&self) {
        self.truncated_total.fetch_add(1, Ordering::Relaxed);
    }
    /// Increments and returns the new count; callers use the returned
    /// value to rate-limit warn! logs (e.g. log every Nth drop).
    pub fn inc_disk_saturated_drop(&self) -> u64 {
        self.disk_saturated_drops_total
            .fetch_add(1, Ordering::Relaxed)
            + 1
    }
    /// Increments and returns the new count; callers use the returned
    /// value to rate-limit warn! logs (e.g. log every Nth drop).
    pub fn inc_http_saturated_drop(&self) -> u64 {
        self.http_saturated_drops_total
            .fetch_add(1, Ordering::Relaxed)
            + 1
    }

    /// Render all metrics in Prometheus text format, appending to `out`.
    /// `in_memory_runs` is passed in because it's read live from the cache.
    pub fn collect(&self, out: &mut String, in_memory_runs: u64) {
        let _ = writeln!(
            out,
            "# HELP oxphp_profiler_runs_total Number of profile runs captured, by activation source."
        );
        let _ = writeln!(out, "# TYPE oxphp_profiler_runs_total counter");
        for (i, label) in SOURCE_LABELS.iter().enumerate() {
            let _ = writeln!(
                out,
                "oxphp_profiler_runs_total{{source=\"{}\"}} {}",
                label,
                self.runs_total[i].load(Ordering::Relaxed)
            );
        }
        let _ = writeln!(
            out,
            "# HELP oxphp_profiler_spans_collected_total Total spans captured across all runs."
        );
        let _ = writeln!(out, "# TYPE oxphp_profiler_spans_collected_total counter");
        let _ = writeln!(
            out,
            "oxphp_profiler_spans_collected_total {}",
            self.spans_collected_total.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            out,
            "# HELP oxphp_profiler_bytes_written_total Profile bytes written to disk, by format."
        );
        let _ = writeln!(out, "# TYPE oxphp_profiler_bytes_written_total counter");
        for (i, label) in FORMAT_LABELS.iter().enumerate() {
            let _ = writeln!(
                out,
                "oxphp_profiler_bytes_written_total{{format=\"{}\"}} {}",
                label,
                self.bytes_written_total[i].load(Ordering::Relaxed)
            );
        }
        let _ = writeln!(
            out,
            "# HELP oxphp_profiler_disk_drops_total Profiles dropped due to rate-limit or IO error."
        );
        let _ = writeln!(out, "# TYPE oxphp_profiler_disk_drops_total counter");
        let _ = writeln!(
            out,
            "oxphp_profiler_disk_drops_total {}",
            self.disk_drops_total.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            out,
            "# HELP oxphp_profiler_http_push_failures_total HTTP push failures after all retries."
        );
        let _ = writeln!(
            out,
            "# TYPE oxphp_profiler_http_push_failures_total counter"
        );
        let _ = writeln!(
            out,
            "oxphp_profiler_http_push_failures_total {}",
            self.http_push_failures_total.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            out,
            "# HELP oxphp_profiler_truncated_total Profile runs where the span tree hit max_spans."
        );
        let _ = writeln!(out, "# TYPE oxphp_profiler_truncated_total counter");
        let _ = writeln!(
            out,
            "oxphp_profiler_truncated_total {}",
            self.truncated_total.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            out,
            "# HELP oxphp_profiler_disk_saturated_drops_total Runs dropped at the spawn fan-out because the disk admission semaphore was saturated."
        );
        let _ = writeln!(
            out,
            "# TYPE oxphp_profiler_disk_saturated_drops_total counter"
        );
        let _ = writeln!(
            out,
            "oxphp_profiler_disk_saturated_drops_total {}",
            self.disk_saturated_drops_total.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            out,
            "# HELP oxphp_profiler_http_saturated_drops_total Runs dropped at the spawn fan-out because the HTTP admission semaphore was saturated."
        );
        let _ = writeln!(
            out,
            "# TYPE oxphp_profiler_http_saturated_drops_total counter"
        );
        let _ = writeln!(
            out,
            "oxphp_profiler_http_saturated_drops_total {}",
            self.http_saturated_drops_total.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            out,
            "# HELP oxphp_profiler_in_memory_runs Current number of runs held in the LRU."
        );
        let _ = writeln!(out, "# TYPE oxphp_profiler_in_memory_runs gauge");
        let _ = writeln!(out, "oxphp_profiler_in_memory_runs {}", in_memory_runs);
    }
}

const FORMAT_LABELS: [&str; FORMAT_COUNT] = ["xhprof", "speedscope", "pprof", "collapsed"];
const SOURCE_LABELS: [&str; SOURCE_COUNT] = ["header", "cookie", "query", "sample"];

fn format_index(f: OutputFormat) -> usize {
    match f {
        OutputFormat::Xhprof => 0,
        OutputFormat::Speedscope => 1,
        OutputFormat::Pprof => 2,
        OutputFormat::Collapsed => 3,
    }
}

fn source_index(s: ActivationSource) -> usize {
    match s {
        ActivationSource::Header => 0,
        ActivationSource::Cookie => 1,
        ActivationSource::Query => 2,
        ActivationSource::SampleRate => 3,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counters_increment() {
        let m = StorageMetrics::new();
        m.inc_runs(ActivationSource::Header);
        m.add_spans(42);
        m.add_bytes(OutputFormat::Xhprof, 1024);
        m.inc_disk_drop();
        m.inc_http_push_failure();
        m.inc_truncated();
        assert_eq!(m.runs_total[0].load(Ordering::Relaxed), 1);
        assert_eq!(m.spans_collected_total.load(Ordering::Relaxed), 42);
        assert_eq!(m.bytes_written_total[0].load(Ordering::Relaxed), 1024);
        assert_eq!(m.disk_drops_total.load(Ordering::Relaxed), 1);
        assert_eq!(m.http_push_failures_total.load(Ordering::Relaxed), 1);
        assert_eq!(m.truncated_total.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn prometheus_output_has_all_names() {
        let m = StorageMetrics::new();
        m.inc_runs(ActivationSource::Header);
        m.add_spans(3);
        m.add_bytes(OutputFormat::Collapsed, 77);
        let mut out = String::new();
        m.collect(&mut out, 2);
        for expected in [
            "oxphp_profiler_runs_total{source=\"header\"} 1",
            "oxphp_profiler_spans_collected_total 3",
            "oxphp_profiler_bytes_written_total{format=\"collapsed\"} 77",
            "oxphp_profiler_disk_drops_total 0",
            "oxphp_profiler_http_push_failures_total 0",
            "oxphp_profiler_truncated_total 0",
            "oxphp_profiler_in_memory_runs 2",
        ] {
            assert!(out.contains(expected), "missing: {expected}\n{out}");
        }
    }
}
