use std::fmt::Write;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use http::Method;

// ── Worker Mode Metrics ──────────────────────────────────────

/// Per-worker stats shared between the worker thread and the metrics collector.
pub struct WorkerStats {
    pub memory_bytes: AtomicU64,
    pub requests_done: AtomicU64,
    /// Unix epoch milliseconds when the worker thread was spawned.
    pub spawn_time_ms: AtomicU64,
    pub active: AtomicBool,
}

impl Default for WorkerStats {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkerStats {
    pub fn new() -> Self {
        Self {
            memory_bytes: AtomicU64::new(0),
            requests_done: AtomicU64::new(0),
            spawn_time_ms: AtomicU64::new(0),
            active: AtomicBool::new(false),
        }
    }
}

/// Histogram bucket boundaries for request duration (microseconds).
const DURATION_BUCKET_BOUNDS: [u64; 9] =
    [100, 250, 500, 1_000, 2_500, 5_000, 10_000, 25_000, 50_000];

/// Global worker mode metrics shared across all worker threads.
pub struct WorkerMetrics {
    pub requests_handled_total: AtomicU64,
    pub recycles_total: AtomicU64,
    pub recycles_max_requests: AtomicU64,
    pub recycles_max_memory: AtomicU64,
    pub recycles_error: AtomicU64,
    pub soft_resets_total: AtomicU64,
    /// Histogram buckets: 9 bounded + 1 +Inf
    pub duration_buckets: [AtomicU64; 10],
    pub duration_sum_us: AtomicU64,
    pub duration_count: AtomicU64,
    /// Per-worker stats slots (indexed by worker_id % len).
    pub slots: Box<[Arc<WorkerStats>]>,
}

impl WorkerMetrics {
    pub fn new(max_workers: usize) -> Self {
        let slots: Vec<Arc<WorkerStats>> = (0..max_workers)
            .map(|_| Arc::new(WorkerStats::new()))
            .collect();
        Self {
            requests_handled_total: AtomicU64::new(0),
            recycles_total: AtomicU64::new(0),
            recycles_max_requests: AtomicU64::new(0),
            recycles_max_memory: AtomicU64::new(0),
            recycles_error: AtomicU64::new(0),
            soft_resets_total: AtomicU64::new(0),
            duration_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            duration_sum_us: AtomicU64::new(0),
            duration_count: AtomicU64::new(0),
            slots: slots.into_boxed_slice(),
        }
    }

    /// Record a request duration in the histogram.
    /// Each value lands in exactly one bucket (the smallest bound >= duration_us).
    /// Rendering accumulates for Prometheus cumulative output.
    pub fn record_duration(&self, duration_us: u64) {
        let mut placed = false;
        for (i, &bound) in DURATION_BUCKET_BOUNDS.iter().enumerate() {
            if duration_us <= bound {
                self.duration_buckets[i].fetch_add(1, Ordering::Relaxed);
                placed = true;
                break;
            }
        }
        if !placed {
            // > 50_000us → +Inf bucket
            self.duration_buckets[9].fetch_add(1, Ordering::Relaxed);
        }
        self.duration_sum_us
            .fetch_add(duration_us, Ordering::Relaxed);
        self.duration_count.fetch_add(1, Ordering::Relaxed);
    }

    /// Record a worker recycle with reason.
    pub fn record_recycle(&self, exit_reason: u8) {
        self.recycles_total.fetch_add(1, Ordering::Relaxed);
        match exit_reason {
            1 => {
                self.recycles_max_requests.fetch_add(1, Ordering::Relaxed);
            }
            2 => {
                self.recycles_max_memory.fetch_add(1, Ordering::Relaxed);
            }
            3 => {
                self.recycles_error.fetch_add(1, Ordering::Relaxed);
            }
            _ => {} // 0 = shutdown, not a recycle reason
        }
    }
}

/// Lock-free atomic metrics counters for the server.
/// All operations use `Relaxed` ordering — counters are approximate and don't
/// need happens-before guarantees with other data.
pub struct Metrics {
    start_time: Instant,
    total_requests: AtomicU64,
    active_connections: AtomicUsize,
    pending_requests: AtomicUsize,
    dropped_requests: AtomicU64,
    requests_by_method: [AtomicU64; 10],
    responses_by_status_class: [AtomicU64; 5],
    total_response_time_us: AtomicU64,
    busy_workers: AtomicUsize,
    workers_current: AtomicUsize,
    workers_min: AtomicUsize,
    workers_max: AtomicUsize,
    workers_idle: AtomicUsize,
    workers_spawned_total: AtomicU64,
    workers_retired_total: AtomicU64,
    worker_metrics: std::sync::OnceLock<Arc<WorkerMetrics>>,
}

const METHOD_LABELS: [&str; 10] = [
    "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "CONNECT", "QUERY", "OTHER",
];

fn method_index(method: &Method) -> usize {
    match *method {
        Method::GET => 0,
        Method::POST => 1,
        Method::PUT => 2,
        Method::DELETE => 3,
        Method::PATCH => 4,
        Method::HEAD => 5,
        Method::OPTIONS => 6,
        Method::CONNECT => 7,
        _ if method.as_str() == "QUERY" => 8,
        _ => 9,
    }
}

fn status_class_index(status: u16) -> usize {
    let class = (status / 100) as usize;
    if (1..=5).contains(&class) {
        class - 1
    } else {
        4 // unknown → 5xx bucket
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            start_time: Instant::now(),
            total_requests: AtomicU64::new(0),
            active_connections: AtomicUsize::new(0),
            pending_requests: AtomicUsize::new(0),
            dropped_requests: AtomicU64::new(0),
            requests_by_method: std::array::from_fn(|_| AtomicU64::new(0)),
            responses_by_status_class: std::array::from_fn(|_| AtomicU64::new(0)),
            total_response_time_us: AtomicU64::new(0),
            busy_workers: AtomicUsize::new(0),
            workers_current: AtomicUsize::new(0),
            workers_min: AtomicUsize::new(0),
            workers_max: AtomicUsize::new(0),
            workers_idle: AtomicUsize::new(0),
            workers_spawned_total: AtomicU64::new(0),
            workers_retired_total: AtomicU64::new(0),
            worker_metrics: std::sync::OnceLock::new(),
        }
    }

    pub fn record_request(&self, method: &Method) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.requests_by_method[method_index(method)].fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_response(&self, status: u16, duration: Duration) {
        self.responses_by_status_class[status_class_index(status)].fetch_add(1, Ordering::Relaxed);
        self.total_response_time_us
            .fetch_add(duration.as_micros() as u64, Ordering::Relaxed);
    }

    pub fn connection_opened(&self) {
        self.active_connections.fetch_add(1, Ordering::Relaxed);
    }

    pub fn connection_closed(&self) {
        self.active_connections.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn request_queued(&self) {
        self.pending_requests.fetch_add(1, Ordering::Relaxed);
        self.busy_workers.fetch_add(1, Ordering::Relaxed);
    }

    pub fn request_dequeued(&self) {
        self.pending_requests.fetch_sub(1, Ordering::Relaxed);
        self.busy_workers.fetch_sub(1, Ordering::Relaxed);
    }

    pub fn request_dropped(&self) {
        self.dropped_requests.fetch_add(1, Ordering::Relaxed);
    }

    pub fn set_workers_current(&self, n: usize) {
        self.workers_current.store(n, Ordering::Relaxed);
    }

    pub fn set_workers_min(&self, n: usize) {
        self.workers_min.store(n, Ordering::Relaxed);
    }

    pub fn set_workers_max(&self, n: usize) {
        self.workers_max.store(n, Ordering::Relaxed);
    }

    pub fn set_workers_idle(&self, n: usize) {
        self.workers_idle.store(n, Ordering::Relaxed);
    }

    pub fn worker_spawned(&self) {
        self.workers_spawned_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn worker_retired(&self) {
        self.workers_retired_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn workers_current(&self) -> usize {
        self.workers_current.load(Ordering::Relaxed)
    }

    pub fn set_worker_metrics(&self, wm: Arc<WorkerMetrics>) {
        self.worker_metrics.set(wm).ok();
    }

    pub fn worker_metrics(&self) -> Option<&Arc<WorkerMetrics>> {
        self.worker_metrics.get()
    }

    pub fn uptime(&self) -> Duration {
        self.start_time.elapsed()
    }

    pub fn total_requests(&self) -> u64 {
        self.total_requests.load(Ordering::Relaxed)
    }

    pub fn active_connections(&self) -> usize {
        self.active_connections.load(Ordering::Relaxed)
    }

    pub fn to_prometheus(&self) -> String {
        let mut out = String::with_capacity(1024);

        let _ = writeln!(out, "# HELP oxphp_uptime_seconds Server uptime in seconds.");
        let _ = writeln!(out, "# TYPE oxphp_uptime_seconds gauge");
        let _ = writeln!(out, "oxphp_uptime_seconds {}", self.uptime().as_secs());

        let _ = writeln!(out, "# HELP oxphp_requests_total Total HTTP requests.");
        let _ = writeln!(out, "# TYPE oxphp_requests_total counter");
        let _ = writeln!(
            out,
            "oxphp_requests_total {}",
            self.total_requests.load(Ordering::Relaxed)
        );

        let _ = writeln!(
            out,
            "# HELP oxphp_requests_by_method_total Requests by HTTP method."
        );
        let _ = writeln!(out, "# TYPE oxphp_requests_by_method_total counter");
        for (i, label) in METHOD_LABELS.iter().enumerate() {
            let count = self.requests_by_method[i].load(Ordering::Relaxed);
            if count > 0 {
                let _ = writeln!(
                    out,
                    "oxphp_requests_by_method_total{{method=\"{label}\"}} {count}"
                );
            }
        }

        let _ = writeln!(
            out,
            "# HELP oxphp_responses_by_status_total Responses by status class."
        );
        let _ = writeln!(out, "# TYPE oxphp_responses_by_status_total counter");
        let status_labels = ["1xx", "2xx", "3xx", "4xx", "5xx"];
        for (i, label) in status_labels.iter().enumerate() {
            let count = self.responses_by_status_class[i].load(Ordering::Relaxed);
            if count > 0 {
                let _ = writeln!(
                    out,
                    "oxphp_responses_by_status_total{{status=\"{label}\"}} {count}"
                );
            }
        }

        let _ = writeln!(
            out,
            "# HELP oxphp_active_connections Current active connections."
        );
        let _ = writeln!(out, "# TYPE oxphp_active_connections gauge");
        let _ = writeln!(
            out,
            "oxphp_active_connections {}",
            self.active_connections.load(Ordering::Relaxed)
        );

        let _ = writeln!(
            out,
            "# HELP oxphp_pending_requests Requests waiting in queue."
        );
        let _ = writeln!(out, "# TYPE oxphp_pending_requests gauge");
        let _ = writeln!(
            out,
            "oxphp_pending_requests {}",
            self.pending_requests.load(Ordering::Relaxed)
        );

        let _ = writeln!(
            out,
            "# HELP oxphp_dropped_requests_total Requests dropped (503)."
        );
        let _ = writeln!(out, "# TYPE oxphp_dropped_requests_total counter");
        let _ = writeln!(
            out,
            "oxphp_dropped_requests_total {}",
            self.dropped_requests.load(Ordering::Relaxed)
        );

        let _ = writeln!(
            out,
            "# HELP oxphp_response_time_us_total Total response time in microseconds."
        );
        let _ = writeln!(out, "# TYPE oxphp_response_time_us_total counter");
        let _ = writeln!(
            out,
            "oxphp_response_time_us_total {}",
            self.total_response_time_us.load(Ordering::Relaxed)
        );

        let _ = writeln!(
            out,
            "# HELP oxphp_busy_workers Currently busy worker threads."
        );
        let _ = writeln!(out, "# TYPE oxphp_busy_workers gauge");
        let _ = writeln!(
            out,
            "oxphp_busy_workers {}",
            self.busy_workers.load(Ordering::Relaxed)
        );

        let _ = writeln!(
            out,
            "# HELP oxphp_workers_current Current number of worker threads."
        );
        let _ = writeln!(out, "# TYPE oxphp_workers_current gauge");
        let _ = writeln!(
            out,
            "oxphp_workers_current {}",
            self.workers_current.load(Ordering::Relaxed)
        );

        let _ = writeln!(out, "# HELP oxphp_workers_min Minimum worker thread count.");
        let _ = writeln!(out, "# TYPE oxphp_workers_min gauge");
        let _ = writeln!(
            out,
            "oxphp_workers_min {}",
            self.workers_min.load(Ordering::Relaxed)
        );

        let _ = writeln!(out, "# HELP oxphp_workers_max Maximum worker thread count.");
        let _ = writeln!(out, "# TYPE oxphp_workers_max gauge");
        let _ = writeln!(
            out,
            "oxphp_workers_max {}",
            self.workers_max.load(Ordering::Relaxed)
        );

        let _ = writeln!(
            out,
            "# HELP oxphp_workers_idle Currently idle worker threads."
        );
        let _ = writeln!(out, "# TYPE oxphp_workers_idle gauge");
        let _ = writeln!(
            out,
            "oxphp_workers_idle {}",
            self.workers_idle.load(Ordering::Relaxed)
        );

        let _ = writeln!(
            out,
            "# HELP oxphp_workers_spawned_total Total workers spawned."
        );
        let _ = writeln!(out, "# TYPE oxphp_workers_spawned_total counter");
        let _ = writeln!(
            out,
            "oxphp_workers_spawned_total {}",
            self.workers_spawned_total.load(Ordering::Relaxed)
        );

        let _ = writeln!(
            out,
            "# HELP oxphp_workers_retired_total Total workers retired."
        );
        let _ = writeln!(out, "# TYPE oxphp_workers_retired_total counter");
        let _ = writeln!(
            out,
            "oxphp_workers_retired_total {}",
            self.workers_retired_total.load(Ordering::Relaxed)
        );

        // ── Worker Mode Metrics ──
        if let Some(wm) = self.worker_metrics.get() {
            let _ = writeln!(
                out,
                "# HELP oxphp_worker_mode_enabled Whether worker mode is active."
            );
            let _ = writeln!(out, "# TYPE oxphp_worker_mode_enabled gauge");
            let _ = writeln!(out, "oxphp_worker_mode_enabled 1");

            let _ = writeln!(out, "# HELP oxphp_worker_requests_handled_total Total requests processed by worker mode.");
            let _ = writeln!(out, "# TYPE oxphp_worker_requests_handled_total counter");
            let _ = writeln!(
                out,
                "oxphp_worker_requests_handled_total {}",
                wm.requests_handled_total.load(Ordering::Relaxed)
            );

            let _ = writeln!(
                out,
                "# HELP oxphp_worker_recycles_total Total worker recycles."
            );
            let _ = writeln!(out, "# TYPE oxphp_worker_recycles_total counter");
            let _ = writeln!(
                out,
                "oxphp_worker_recycles_total {}",
                wm.recycles_total.load(Ordering::Relaxed)
            );

            let _ = writeln!(
                out,
                "# HELP oxphp_worker_recycles_by_reason_total Worker recycles by reason."
            );
            let _ = writeln!(out, "# TYPE oxphp_worker_recycles_by_reason_total counter");
            let max_req = wm.recycles_max_requests.load(Ordering::Relaxed);
            let max_mem = wm.recycles_max_memory.load(Ordering::Relaxed);
            let error = wm.recycles_error.load(Ordering::Relaxed);
            if max_req > 0 {
                let _ = writeln!(
                    out,
                    "oxphp_worker_recycles_by_reason_total{{reason=\"max_requests\"}} {max_req}"
                );
            }
            if max_mem > 0 {
                let _ = writeln!(
                    out,
                    "oxphp_worker_recycles_by_reason_total{{reason=\"max_memory\"}} {max_mem}"
                );
            }
            if error > 0 {
                let _ = writeln!(
                    out,
                    "oxphp_worker_recycles_by_reason_total{{reason=\"error\"}} {error}"
                );
            }

            let _ = writeln!(
                out,
                "# HELP oxphp_worker_soft_resets_total Total soft resets between requests."
            );
            let _ = writeln!(out, "# TYPE oxphp_worker_soft_resets_total counter");
            let _ = writeln!(
                out,
                "oxphp_worker_soft_resets_total {}",
                wm.soft_resets_total.load(Ordering::Relaxed)
            );

            // Per-worker gauges
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            let _ = writeln!(
                out,
                "# HELP oxphp_worker_memory_bytes Current PHP heap per worker."
            );
            let _ = writeln!(out, "# TYPE oxphp_worker_memory_bytes gauge");
            let _ = writeln!(
                out,
                "# HELP oxphp_worker_uptime_seconds Time since worker thread spawned."
            );
            let _ = writeln!(out, "# TYPE oxphp_worker_uptime_seconds gauge");
            let _ = writeln!(
                out,
                "# HELP oxphp_worker_requests_count Requests handled by this worker instance."
            );
            let _ = writeln!(out, "# TYPE oxphp_worker_requests_count gauge");

            for (i, slot) in wm.slots.iter().enumerate() {
                if !slot.active.load(Ordering::Relaxed) {
                    continue;
                }
                let mem = slot.memory_bytes.load(Ordering::Relaxed);
                let reqs = slot.requests_done.load(Ordering::Relaxed);
                let spawn = slot.spawn_time_ms.load(Ordering::Relaxed);
                let uptime_s = now_ms.saturating_sub(spawn) / 1000;

                let _ = writeln!(out, "oxphp_worker_memory_bytes{{worker=\"{i}\"}} {mem}");
                let _ = writeln!(
                    out,
                    "oxphp_worker_uptime_seconds{{worker=\"{i}\"}} {uptime_s}"
                );
                let _ = writeln!(out, "oxphp_worker_requests_count{{worker=\"{i}\"}} {reqs}");
            }

            // Histogram
            let _ = writeln!(
                out,
                "# HELP oxphp_worker_request_duration_us PHP execution time per request."
            );
            let _ = writeln!(out, "# TYPE oxphp_worker_request_duration_us histogram");
            let mut cumulative = 0u64;
            for (i, &bound) in DURATION_BUCKET_BOUNDS.iter().enumerate() {
                cumulative += wm.duration_buckets[i].load(Ordering::Relaxed);
                let _ = writeln!(
                    out,
                    "oxphp_worker_request_duration_us_bucket{{le=\"{bound}\"}} {cumulative}"
                );
            }
            cumulative += wm.duration_buckets[9].load(Ordering::Relaxed);
            let _ = writeln!(
                out,
                "oxphp_worker_request_duration_us_bucket{{le=\"+Inf\"}} {cumulative}"
            );
            let _ = writeln!(
                out,
                "oxphp_worker_request_duration_us_sum {}",
                wm.duration_sum_us.load(Ordering::Relaxed)
            );
            let _ = writeln!(
                out,
                "oxphp_worker_request_duration_us_count {}",
                wm.duration_count.load(Ordering::Relaxed)
            );
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_method_index_mapping() {
        assert_eq!(method_index(&Method::GET), 0);
        assert_eq!(method_index(&Method::POST), 1);
        assert_eq!(method_index(&Method::PUT), 2);
        assert_eq!(method_index(&Method::DELETE), 3);
        assert_eq!(method_index(&Method::PATCH), 4);
        assert_eq!(method_index(&Method::HEAD), 5);
        assert_eq!(method_index(&Method::OPTIONS), 6);
        assert_eq!(method_index(&Method::CONNECT), 7);
        assert_eq!(method_index(&Method::from_bytes(b"QUERY").unwrap()), 8);
        assert_eq!(method_index(&Method::TRACE), 9); // OTHER
    }

    #[test]
    fn test_status_class_index() {
        assert_eq!(status_class_index(100), 0);
        assert_eq!(status_class_index(200), 1);
        assert_eq!(status_class_index(301), 2);
        assert_eq!(status_class_index(404), 3);
        assert_eq!(status_class_index(500), 4);
        assert_eq!(status_class_index(0), 4); // unknown → 5xx
    }

    #[test]
    fn test_record_request() {
        let m = Metrics::new();
        m.record_request(&Method::GET);
        m.record_request(&Method::GET);
        m.record_request(&Method::POST);

        assert_eq!(m.total_requests(), 3);
        assert_eq!(m.requests_by_method[0].load(Ordering::Relaxed), 2);
        assert_eq!(m.requests_by_method[1].load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_record_response() {
        let m = Metrics::new();
        m.record_response(200, Duration::from_micros(500));
        m.record_response(404, Duration::from_micros(100));
        m.record_response(500, Duration::from_micros(200));

        assert_eq!(m.responses_by_status_class[1].load(Ordering::Relaxed), 1); // 2xx
        assert_eq!(m.responses_by_status_class[3].load(Ordering::Relaxed), 1); // 4xx
        assert_eq!(m.responses_by_status_class[4].load(Ordering::Relaxed), 1); // 5xx
        assert_eq!(m.total_response_time_us.load(Ordering::Relaxed), 800);
    }

    #[test]
    fn test_connections() {
        let m = Metrics::new();
        m.connection_opened();
        m.connection_opened();
        assert_eq!(m.active_connections(), 2);
        m.connection_closed();
        assert_eq!(m.active_connections(), 1);
    }

    #[test]
    fn test_queue_metrics() {
        let m = Metrics::new();
        m.request_queued();
        m.request_queued();
        assert_eq!(m.pending_requests.load(Ordering::Relaxed), 2);
        assert_eq!(m.busy_workers.load(Ordering::Relaxed), 2);
        m.request_dequeued();
        assert_eq!(m.pending_requests.load(Ordering::Relaxed), 1);
        m.request_dropped();
        assert_eq!(m.dropped_requests.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_worker_metrics() {
        let m = Metrics::new();
        m.set_workers_current(4);
        m.set_workers_min(2);
        m.set_workers_max(16);
        m.set_workers_idle(2);
        m.worker_spawned();
        m.worker_spawned();
        m.worker_retired();

        assert_eq!(m.workers_current(), 4);
        assert_eq!(m.workers_min.load(Ordering::Relaxed), 2);
        assert_eq!(m.workers_max.load(Ordering::Relaxed), 16);
        assert_eq!(m.workers_idle.load(Ordering::Relaxed), 2);
        assert_eq!(m.workers_spawned_total.load(Ordering::Relaxed), 2);
        assert_eq!(m.workers_retired_total.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_worker_metrics_prometheus() {
        let m = Metrics::new();
        m.set_workers_current(8);
        m.set_workers_min(2);
        m.set_workers_max(16);

        let output = m.to_prometheus();
        assert!(output.contains("oxphp_workers_current 8"));
        assert!(output.contains("oxphp_workers_min 2"));
        assert!(output.contains("oxphp_workers_max 16"));
    }

    #[test]
    fn test_to_prometheus() {
        let m = Metrics::new();
        m.record_request(&Method::GET);
        m.record_response(200, Duration::from_micros(1000));

        let output = m.to_prometheus();
        assert!(output.contains("oxphp_requests_total 1"));
        assert!(output.contains("oxphp_requests_by_method_total{method=\"GET\"} 1"));
        assert!(output.contains("oxphp_responses_by_status_total{status=\"2xx\"} 1"));
        assert!(output.contains("oxphp_response_time_us_total 1000"));
    }

    // ── WorkerMetrics tests ──

    #[test]
    fn test_worker_metrics_counters() {
        let wm = WorkerMetrics::new(4);
        wm.requests_handled_total.fetch_add(5, Ordering::Relaxed);
        wm.soft_resets_total.fetch_add(5, Ordering::Relaxed);
        assert_eq!(wm.requests_handled_total.load(Ordering::Relaxed), 5);
        assert_eq!(wm.soft_resets_total.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn test_worker_metrics_recycle() {
        let wm = WorkerMetrics::new(2);
        wm.record_recycle(1); // max_requests
        wm.record_recycle(2); // max_memory
        wm.record_recycle(3); // error
        wm.record_recycle(0); // shutdown — not counted as reason

        assert_eq!(wm.recycles_total.load(Ordering::Relaxed), 4);
        assert_eq!(wm.recycles_max_requests.load(Ordering::Relaxed), 1);
        assert_eq!(wm.recycles_max_memory.load(Ordering::Relaxed), 1);
        assert_eq!(wm.recycles_error.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn test_worker_metrics_histogram_bucketing() {
        let wm = WorkerMetrics::new(1);

        // 50us → bucket[0] (le=100)
        wm.record_duration(50);
        // 200us → bucket[1] (le=250)
        wm.record_duration(200);
        // 100us → bucket[0] (le=100)
        wm.record_duration(100);
        // 60000us → bucket[9] (+Inf)
        wm.record_duration(60000);

        assert_eq!(wm.duration_buckets[0].load(Ordering::Relaxed), 2); // <=100
        assert_eq!(wm.duration_buckets[1].load(Ordering::Relaxed), 1); // <=250
        assert_eq!(wm.duration_buckets[9].load(Ordering::Relaxed), 1); // +Inf
        assert_eq!(wm.duration_count.load(Ordering::Relaxed), 4);
        assert_eq!(
            wm.duration_sum_us.load(Ordering::Relaxed),
            50 + 200 + 100 + 60000
        );
    }

    #[test]
    fn test_worker_metrics_per_worker_stats() {
        let wm = WorkerMetrics::new(4);

        // Simulate worker 0 activity
        wm.slots[0].active.store(true, Ordering::Relaxed);
        wm.slots[0].memory_bytes.store(2_000_000, Ordering::Relaxed);
        wm.slots[0].requests_done.store(42, Ordering::Relaxed);
        wm.slots[0].spawn_time_ms.store(1000, Ordering::Relaxed);

        assert!(wm.slots[0].active.load(Ordering::Relaxed));
        assert_eq!(wm.slots[0].memory_bytes.load(Ordering::Relaxed), 2_000_000);
        assert_eq!(wm.slots[0].requests_done.load(Ordering::Relaxed), 42);
    }

    #[test]
    fn test_worker_metrics_prometheus_output() {
        let m = Metrics::new();
        let wm = Arc::new(WorkerMetrics::new(2));

        // Simulate some activity
        wm.requests_handled_total.fetch_add(10, Ordering::Relaxed);
        wm.recycles_total.fetch_add(1, Ordering::Relaxed);
        wm.recycles_max_requests.fetch_add(1, Ordering::Relaxed);
        wm.soft_resets_total.fetch_add(10, Ordering::Relaxed);
        wm.slots[0].active.store(true, Ordering::Relaxed);
        wm.slots[0].memory_bytes.store(1_048_576, Ordering::Relaxed);
        wm.slots[0].requests_done.store(5, Ordering::Relaxed);

        // Record some durations
        wm.record_duration(150);
        wm.record_duration(3000);

        m.set_worker_metrics(wm);
        let output = m.to_prometheus();

        assert!(output.contains("oxphp_worker_mode_enabled 1"));
        assert!(output.contains("oxphp_worker_requests_handled_total 10"));
        assert!(output.contains("oxphp_worker_recycles_total 1"));
        assert!(output.contains("oxphp_worker_recycles_by_reason_total{reason=\"max_requests\"} 1"));
        assert!(output.contains("oxphp_worker_soft_resets_total 10"));
        assert!(output.contains("oxphp_worker_memory_bytes{worker=\"0\"} 1048576"));
        assert!(output.contains("oxphp_worker_requests_count{worker=\"0\"} 5"));
        // Worker 1 is not active, so should not appear
        assert!(!output.contains("worker=\"1\""));
        // Histogram
        assert!(output.contains("oxphp_worker_request_duration_us_bucket{le=\"100\"} 0"));
        assert!(output.contains("oxphp_worker_request_duration_us_bucket{le=\"250\"} 1"));
        assert!(output.contains("oxphp_worker_request_duration_us_bucket{le=\"+Inf\"} 2"));
        assert!(output.contains("oxphp_worker_request_duration_us_sum 3150"));
        assert!(output.contains("oxphp_worker_request_duration_us_count 2"));
    }

    #[test]
    fn test_no_worker_metrics_no_output() {
        let m = Metrics::new();
        let output = m.to_prometheus();
        assert!(!output.contains("oxphp_worker_mode_enabled"));
        assert!(!output.contains("oxphp_worker_requests_handled"));
    }
}
