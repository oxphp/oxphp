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
    pub recycles_scheduled: AtomicU64,
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
            recycles_scheduled: AtomicU64::new(0),
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
                self.recycles_scheduled.fetch_add(1, Ordering::Relaxed);
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

/// Histogram bucket boundaries for overall request duration (microseconds).
/// Wider range than worker-mode buckets to cover static files + PHP.
const REQUEST_DURATION_BUCKET_BOUNDS: [u64; 12] = [
    100, 500, 1_000, 2_500, 5_000, 10_000, 25_000, 50_000, 100_000, 250_000, 500_000, 1_000_000,
];

/// Histogram bucket boundaries for queue wait time (microseconds).
const QUEUE_WAIT_BUCKET_BOUNDS: [u64; 9] = [50, 100, 250, 500, 1_000, 2_500, 5_000, 10_000, 50_000];

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
    workers_spawned_total: AtomicU64,
    workers_retired_total: AtomicU64,
    worker_metrics: std::sync::OnceLock<Arc<WorkerMetrics>>,

    // ── New metrics ──
    /// Request duration histogram (all requests, not just worker mode).
    request_duration_buckets: [AtomicU64; 13], // 12 bounded + 1 +Inf
    // _sum reuses total_response_time_us; _count derived from responses_by_status_class sum.
    /// Total request body bytes received.
    request_bytes_total: AtomicU64,
    /// Total response body bytes sent.
    response_bytes_total: AtomicU64,

    /// Queue wait time histogram (time between queue submit and worker pickup).
    queue_wait_buckets: [AtomicU64; 10], // 9 bounded + 1 +Inf
    queue_wait_sum_us: AtomicU64,
    queue_wait_count: AtomicU64,

    /// Requests rejected by rate limiter (429).
    rate_limited_total: AtomicU64,

    /// Requests denied by PHP_DENY_PATHS.
    php_deny_total: AtomicU64,

    /// Static file cache hits and misses.
    static_cache_hits: AtomicU64,
    static_cache_misses: AtomicU64,

    /// Responses compressed with brotli.
    compressed_responses_total: AtomicU64,
    /// Bytes saved by compression (original - compressed).
    compression_bytes_saved_total: AtomicU64,

    // ── Async pool metrics ──
    async_tasks_dispatched: AtomicU64,
    async_tasks_completed: AtomicU64,
    async_tasks_failed: AtomicU64,
    async_tasks_cancelled: AtomicU64,
    async_tasks_rejected: AtomicU64,
    /// Workers left running past an `oxphp_async_await_race` /
    /// `oxphp_async_await_any` timeout. Cancel flag is signalled but the
    /// worker keeps running PHP code on its OS thread. Each stranded
    /// worker can extend RSHUTDOWN by up to 5s (the per-promise budget
    /// in `cleanup_outstanding_promises_callback`). Watch this counter
    /// to size that risk.
    async_tasks_stranded: AtomicU64,

    // ── Per-worker observability (supervisor-driven) ──
    /// Age of the in-flight request per worker, in microseconds.
    /// Written each scan; idle workers carry 0.
    pub worker_request_age_us: Vec<AtomicU64>,
    /// Per-worker count of supervisor scans that observed an
    /// above-threshold request age.
    pub worker_long_running_total: Vec<AtomicU64>,
    /// Per-worker stuck classification counters (one Vec per kind).
    pub worker_stuck_total_io: Vec<AtomicU64>,
    pub worker_stuck_total_c_call: Vec<AtomicU64>,
    pub worker_stuck_total_cpu: Vec<AtomicU64>,

    /// Cancelled-request counters by reason.
    pub request_cancelled_client_abort: AtomicU64,
    pub request_cancelled_timeout: AtomicU64,
    pub request_cancelled_shutdown: AtomicU64,
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

/// Place a value into the correct histogram bucket. Buckets array must have
/// `bounds.len() + 1` elements (bounded slots + one +Inf slot).
#[inline]
fn record_histogram(buckets: &[AtomicU64], bounds: &[u64], value: u64) {
    for (i, &bound) in bounds.iter().enumerate() {
        if value <= bound {
            buckets[i].fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
    buckets[bounds.len()].fetch_add(1, Ordering::Relaxed);
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

impl Metrics {
    pub fn new() -> Self {
        Self::new_with_workers(0)
    }

    /// Construct with per-worker observability vectors sized to `n`.
    /// `Metrics::new()` keeps the vectors empty; the supervisor and
    /// helpers no-op for out-of-range indices, so legacy callers stay
    /// correct while production initialises with the real worker count.
    pub fn new_with_workers(n: usize) -> Self {
        let mk_vec = || (0..n).map(|_| AtomicU64::new(0)).collect();
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
            workers_spawned_total: AtomicU64::new(0),
            workers_retired_total: AtomicU64::new(0),
            worker_metrics: std::sync::OnceLock::new(),
            request_duration_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            request_bytes_total: AtomicU64::new(0),
            response_bytes_total: AtomicU64::new(0),
            queue_wait_buckets: std::array::from_fn(|_| AtomicU64::new(0)),
            queue_wait_sum_us: AtomicU64::new(0),
            queue_wait_count: AtomicU64::new(0),
            rate_limited_total: AtomicU64::new(0),
            php_deny_total: AtomicU64::new(0),
            static_cache_hits: AtomicU64::new(0),
            static_cache_misses: AtomicU64::new(0),
            compressed_responses_total: AtomicU64::new(0),
            compression_bytes_saved_total: AtomicU64::new(0),
            async_tasks_dispatched: AtomicU64::new(0),
            async_tasks_completed: AtomicU64::new(0),
            async_tasks_failed: AtomicU64::new(0),
            async_tasks_cancelled: AtomicU64::new(0),
            async_tasks_rejected: AtomicU64::new(0),
            async_tasks_stranded: AtomicU64::new(0),
            worker_request_age_us: mk_vec(),
            worker_long_running_total: mk_vec(),
            worker_stuck_total_io: mk_vec(),
            worker_stuck_total_c_call: mk_vec(),
            worker_stuck_total_cpu: mk_vec(),
            request_cancelled_client_abort: AtomicU64::new(0),
            request_cancelled_timeout: AtomicU64::new(0),
            request_cancelled_shutdown: AtomicU64::new(0),
        }
    }

    pub fn observe_age(&self, worker_id: usize, age_us: u64) {
        if let Some(a) = self.worker_request_age_us.get(worker_id) {
            a.store(age_us, Ordering::Relaxed);
        }
    }

    pub fn observe_long_running(&self, worker_id: usize) {
        if let Some(c) = self.worker_long_running_total.get(worker_id) {
            c.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn observe_stuck(&self, worker_id: usize, kind: crate::php::supervisor::StuckKind) {
        let v = match kind {
            crate::php::supervisor::StuckKind::Io => &self.worker_stuck_total_io,
            crate::php::supervisor::StuckKind::CCall => &self.worker_stuck_total_c_call,
            crate::php::supervisor::StuckKind::Cpu => &self.worker_stuck_total_cpu,
        };
        if let Some(c) = v.get(worker_id) {
            c.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Increment the counter that matches `reason` (1=client_abort,
    /// 2=timeout, 3=shutdown). Other values are ignored — reason 4
    /// (stuck) is reserved and reason 5 (user) is intentionally not
    /// emitted today.
    pub fn observe_cancelled(&self, reason: u8) {
        match reason {
            1 => {
                self.request_cancelled_client_abort
                    .fetch_add(1, Ordering::Relaxed);
            }
            2 => {
                self.request_cancelled_timeout
                    .fetch_add(1, Ordering::Relaxed);
            }
            3 => {
                self.request_cancelled_shutdown
                    .fetch_add(1, Ordering::Relaxed);
            }
            _ => {}
        }
    }

    pub fn record_request(&self, method: &Method) {
        self.total_requests.fetch_add(1, Ordering::Relaxed);
        self.requests_by_method[method_index(method)].fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_response(
        &self,
        status: u16,
        duration: Duration,
        request_body_size: u64,
        response_size: u64,
    ) {
        self.responses_by_status_class[status_class_index(status)].fetch_add(1, Ordering::Relaxed);
        let duration_us = duration.as_micros() as u64;
        // total_response_time_us doubles as histogram _sum (same value, one atomic)
        self.total_response_time_us
            .fetch_add(duration_us, Ordering::Relaxed);
        record_histogram(
            &self.request_duration_buckets,
            &REQUEST_DURATION_BUCKET_BOUNDS,
            duration_us,
        );
        self.request_bytes_total
            .fetch_add(request_body_size, Ordering::Relaxed);
        self.response_bytes_total
            .fetch_add(response_size, Ordering::Relaxed);
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

    pub fn worker_spawned(&self) {
        self.workers_spawned_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn worker_retired(&self) {
        self.workers_retired_total.fetch_add(1, Ordering::Relaxed);
    }

    /// Record queue wait time in microseconds.
    pub fn record_queue_wait(&self, wait_us: u64) {
        record_histogram(&self.queue_wait_buckets, &QUEUE_WAIT_BUCKET_BOUNDS, wait_us);
        self.queue_wait_sum_us.fetch_add(wait_us, Ordering::Relaxed);
        self.queue_wait_count.fetch_add(1, Ordering::Relaxed);
    }

    pub fn rate_limited(&self) {
        self.rate_limited_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn php_denied(&self) {
        self.php_deny_total.fetch_add(1, Ordering::Relaxed);
    }

    pub fn static_cache_hit(&self) {
        self.static_cache_hits.fetch_add(1, Ordering::Relaxed);
    }

    pub fn static_cache_miss(&self) {
        self.static_cache_misses.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_compression(&self, bytes_saved: u64) {
        self.compressed_responses_total
            .fetch_add(1, Ordering::Relaxed);
        self.compression_bytes_saved_total
            .fetch_add(bytes_saved, Ordering::Relaxed);
    }

    pub fn async_task_dispatched(&self) {
        self.async_tasks_dispatched.fetch_add(1, Ordering::Relaxed);
    }

    pub fn async_task_completed(&self) {
        self.async_tasks_completed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn async_task_failed(&self) {
        self.async_tasks_failed.fetch_add(1, Ordering::Relaxed);
    }

    pub fn async_task_cancelled(&self) {
        self.async_tasks_cancelled.fetch_add(1, Ordering::Relaxed);
    }

    pub fn async_task_rejected(&self) {
        self.async_tasks_rejected.fetch_add(1, Ordering::Relaxed);
    }

    /// Record `n` workers stranded by an `await_race` / `await_any`
    /// timeout — see the field doc for context.
    pub fn async_tasks_stranded(&self, n: u64) {
        self.async_tasks_stranded.fetch_add(n, Ordering::Relaxed);
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
            "# HELP oxphp_dropped_requests_total Requests dropped (529)."
        );
        let _ = writeln!(out, "# TYPE oxphp_dropped_requests_total counter");
        let _ = writeln!(
            out,
            "oxphp_dropped_requests_total {}",
            self.dropped_requests.load(Ordering::Relaxed)
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
            self.workers_current
                .load(Ordering::Relaxed)
                .saturating_sub(self.busy_workers.load(Ordering::Relaxed))
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

        // ── Request duration histogram ──
        let _ = writeln!(
            out,
            "# HELP oxphp_request_duration_us Request duration in microseconds."
        );
        let _ = writeln!(out, "# TYPE oxphp_request_duration_us histogram");
        let mut cumulative = 0u64;
        for (i, &bound) in REQUEST_DURATION_BUCKET_BOUNDS.iter().enumerate() {
            cumulative += self.request_duration_buckets[i].load(Ordering::Relaxed);
            let _ = writeln!(
                out,
                "oxphp_request_duration_us_bucket{{le=\"{bound}\"}} {cumulative}"
            );
        }
        cumulative += self.request_duration_buckets[12].load(Ordering::Relaxed);
        let _ = writeln!(
            out,
            "oxphp_request_duration_us_bucket{{le=\"+Inf\"}} {cumulative}"
        );
        let _ = writeln!(
            out,
            "oxphp_request_duration_us_sum {}",
            self.total_response_time_us.load(Ordering::Relaxed)
        );
        let response_count: u64 = self
            .responses_by_status_class
            .iter()
            .map(|a| a.load(Ordering::Relaxed))
            .sum();
        let _ = writeln!(out, "oxphp_request_duration_us_count {response_count}");

        // ── Bytes ──
        let _ = writeln!(
            out,
            "# HELP oxphp_request_bytes_total Total request body bytes received."
        );
        let _ = writeln!(out, "# TYPE oxphp_request_bytes_total counter");
        let _ = writeln!(
            out,
            "oxphp_request_bytes_total {}",
            self.request_bytes_total.load(Ordering::Relaxed)
        );

        let _ = writeln!(
            out,
            "# HELP oxphp_response_bytes_total Total response body bytes sent."
        );
        let _ = writeln!(out, "# TYPE oxphp_response_bytes_total counter");
        let _ = writeln!(
            out,
            "oxphp_response_bytes_total {}",
            self.response_bytes_total.load(Ordering::Relaxed)
        );

        // ── Queue wait histogram ──
        let _ = writeln!(
            out,
            "# HELP oxphp_queue_wait_us Time waiting in queue before worker pickup."
        );
        let _ = writeln!(out, "# TYPE oxphp_queue_wait_us histogram");
        let mut cumulative = 0u64;
        for (i, &bound) in QUEUE_WAIT_BUCKET_BOUNDS.iter().enumerate() {
            cumulative += self.queue_wait_buckets[i].load(Ordering::Relaxed);
            let _ = writeln!(
                out,
                "oxphp_queue_wait_us_bucket{{le=\"{bound}\"}} {cumulative}"
            );
        }
        cumulative += self.queue_wait_buckets[9].load(Ordering::Relaxed);
        let _ = writeln!(
            out,
            "oxphp_queue_wait_us_bucket{{le=\"+Inf\"}} {cumulative}"
        );
        let _ = writeln!(
            out,
            "oxphp_queue_wait_us_sum {}",
            self.queue_wait_sum_us.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            out,
            "oxphp_queue_wait_us_count {}",
            self.queue_wait_count.load(Ordering::Relaxed)
        );

        // ── Rate limiting ──
        let _ = writeln!(
            out,
            "# HELP oxphp_rate_limited_total Requests rejected by rate limiter."
        );
        let _ = writeln!(out, "# TYPE oxphp_rate_limited_total counter");
        let _ = writeln!(
            out,
            "oxphp_rate_limited_total {}",
            self.rate_limited_total.load(Ordering::Relaxed)
        );

        let _ = writeln!(
            out,
            "# HELP oxphp_php_deny_total Requests blocked by PHP_DENY_PATHS."
        );
        let _ = writeln!(out, "# TYPE oxphp_php_deny_total counter");
        let _ = writeln!(
            out,
            "oxphp_php_deny_total {}",
            self.php_deny_total.load(Ordering::Relaxed)
        );

        // ── Static file cache ──
        let _ = writeln!(
            out,
            "# HELP oxphp_static_cache_hits_total Static file cache hits."
        );
        let _ = writeln!(out, "# TYPE oxphp_static_cache_hits_total counter");
        let _ = writeln!(
            out,
            "oxphp_static_cache_hits_total {}",
            self.static_cache_hits.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            out,
            "# HELP oxphp_static_cache_misses_total Static file cache misses."
        );
        let _ = writeln!(out, "# TYPE oxphp_static_cache_misses_total counter");
        let _ = writeln!(
            out,
            "oxphp_static_cache_misses_total {}",
            self.static_cache_misses.load(Ordering::Relaxed)
        );

        // ── Compression ──
        let _ = writeln!(
            out,
            "# HELP oxphp_compressed_responses_total Responses compressed with brotli."
        );
        let _ = writeln!(out, "# TYPE oxphp_compressed_responses_total counter");
        let _ = writeln!(
            out,
            "oxphp_compressed_responses_total {}",
            self.compressed_responses_total.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            out,
            "# HELP oxphp_compression_bytes_saved_total Bytes saved by compression."
        );
        let _ = writeln!(out, "# TYPE oxphp_compression_bytes_saved_total counter");
        let _ = writeln!(
            out,
            "oxphp_compression_bytes_saved_total {}",
            self.compression_bytes_saved_total.load(Ordering::Relaxed)
        );

        // ── Async pool metrics ──
        let async_dispatched = self.async_tasks_dispatched.load(Ordering::Relaxed);
        if async_dispatched > 0 || self.async_tasks_rejected.load(Ordering::Relaxed) > 0 {
            let _ = writeln!(
                out,
                "# HELP oxphp_async_tasks_dispatched_total Total async tasks dispatched."
            );
            let _ = writeln!(out, "# TYPE oxphp_async_tasks_dispatched_total counter");
            let _ = writeln!(out, "oxphp_async_tasks_dispatched_total {async_dispatched}");

            let _ = writeln!(
                out,
                "# HELP oxphp_async_tasks_completed_total Async tasks completed successfully."
            );
            let _ = writeln!(out, "# TYPE oxphp_async_tasks_completed_total counter");
            let _ = writeln!(
                out,
                "oxphp_async_tasks_completed_total {}",
                self.async_tasks_completed.load(Ordering::Relaxed)
            );

            let _ = writeln!(
                out,
                "# HELP oxphp_async_tasks_failed_total Async tasks that threw exceptions."
            );
            let _ = writeln!(out, "# TYPE oxphp_async_tasks_failed_total counter");
            let _ = writeln!(
                out,
                "oxphp_async_tasks_failed_total {}",
                self.async_tasks_failed.load(Ordering::Relaxed)
            );

            let _ = writeln!(
                out,
                "# HELP oxphp_async_tasks_cancelled_total Async tasks cancelled."
            );
            let _ = writeln!(out, "# TYPE oxphp_async_tasks_cancelled_total counter");
            let _ = writeln!(
                out,
                "oxphp_async_tasks_cancelled_total {}",
                self.async_tasks_cancelled.load(Ordering::Relaxed)
            );

            let _ = writeln!(
                out,
                "# HELP oxphp_async_tasks_rejected_total Async tasks rejected (queue full)."
            );
            let _ = writeln!(out, "# TYPE oxphp_async_tasks_rejected_total counter");
            let _ = writeln!(
                out,
                "oxphp_async_tasks_rejected_total {}",
                self.async_tasks_rejected.load(Ordering::Relaxed)
            );

            let _ = writeln!(
                out,
                "# HELP oxphp_async_tasks_stranded_total Workers left running past an await_race/await_any timeout. Each can extend RSHUTDOWN by up to 5s."
            );
            let _ = writeln!(out, "# TYPE oxphp_async_tasks_stranded_total counter");
            let _ = writeln!(
                out,
                "oxphp_async_tasks_stranded_total {}",
                self.async_tasks_stranded.load(Ordering::Relaxed)
            );
        }

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
            let scheduled = wm.recycles_scheduled.load(Ordering::Relaxed);
            let max_mem = wm.recycles_max_memory.load(Ordering::Relaxed);
            let error = wm.recycles_error.load(Ordering::Relaxed);
            if scheduled > 0 {
                let _ = writeln!(
                    out,
                    "oxphp_worker_recycles_by_reason_total{{reason=\"scheduled\"}} {scheduled}"
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

        // ── Per-worker observability (supervisor) ──
        if !self.worker_request_age_us.is_empty() {
            let _ = writeln!(
                out,
                "# HELP oxphp_worker_request_age_seconds Age of in-flight request per worker."
            );
            let _ = writeln!(out, "# TYPE oxphp_worker_request_age_seconds gauge");
            for (i, age) in self.worker_request_age_us.iter().enumerate() {
                let secs = age.load(Ordering::Relaxed) as f64 / 1_000_000.0;
                let _ = writeln!(
                    out,
                    "oxphp_worker_request_age_seconds{{worker_id=\"{i}\"}} {secs}"
                );
            }

            let _ = writeln!(
                out,
                "# HELP oxphp_worker_long_running_total Supervisor scans observing a request older than the stuck threshold."
            );
            let _ = writeln!(out, "# TYPE oxphp_worker_long_running_total counter");
            for (i, c) in self.worker_long_running_total.iter().enumerate() {
                let _ = writeln!(
                    out,
                    "oxphp_worker_long_running_total{{worker_id=\"{i}\"}} {}",
                    c.load(Ordering::Relaxed)
                );
            }

            let _ = writeln!(
                out,
                "# HELP oxphp_worker_stuck_total Stuck-classification counter per worker and kind."
            );
            let _ = writeln!(out, "# TYPE oxphp_worker_stuck_total counter");
            for i in 0..self.worker_stuck_total_io.len() {
                let _ = writeln!(
                    out,
                    "oxphp_worker_stuck_total{{worker_id=\"{i}\",kind=\"io\"}} {}",
                    self.worker_stuck_total_io[i].load(Ordering::Relaxed)
                );
                let _ = writeln!(
                    out,
                    "oxphp_worker_stuck_total{{worker_id=\"{i}\",kind=\"c_call\"}} {}",
                    self.worker_stuck_total_c_call[i].load(Ordering::Relaxed)
                );
                let _ = writeln!(
                    out,
                    "oxphp_worker_stuck_total{{worker_id=\"{i}\",kind=\"cpu\"}} {}",
                    self.worker_stuck_total_cpu[i].load(Ordering::Relaxed)
                );
            }
        }

        let _ = writeln!(
            out,
            "# HELP oxphp_request_cancelled_total Cancelled requests by reason."
        );
        let _ = writeln!(out, "# TYPE oxphp_request_cancelled_total counter");
        let _ = writeln!(
            out,
            "oxphp_request_cancelled_total{{reason=\"client_abort\"}} {}",
            self.request_cancelled_client_abort.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            out,
            "oxphp_request_cancelled_total{{reason=\"timeout\"}} {}",
            self.request_cancelled_timeout.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            out,
            "oxphp_request_cancelled_total{{reason=\"shutdown\"}} {}",
            self.request_cancelled_shutdown.load(Ordering::Relaxed)
        );

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
        m.record_response(200, Duration::from_micros(500), 100, 2000);
        m.record_response(404, Duration::from_micros(100), 0, 500);
        m.record_response(500, Duration::from_micros(200), 50, 1000);

        assert_eq!(m.responses_by_status_class[1].load(Ordering::Relaxed), 1); // 2xx
        assert_eq!(m.responses_by_status_class[3].load(Ordering::Relaxed), 1); // 4xx
        assert_eq!(m.responses_by_status_class[4].load(Ordering::Relaxed), 1); // 5xx
        assert_eq!(m.total_response_time_us.load(Ordering::Relaxed), 800);
        assert_eq!(m.request_bytes_total.load(Ordering::Relaxed), 150);
        assert_eq!(m.response_bytes_total.load(Ordering::Relaxed), 3500);
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
        m.worker_spawned();
        m.worker_spawned();
        m.worker_retired();

        assert_eq!(m.workers_current(), 4);
        assert_eq!(m.workers_min.load(Ordering::Relaxed), 2);
        assert_eq!(m.workers_max.load(Ordering::Relaxed), 16);
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
        // workers_idle is computed as workers_current - busy_workers
        assert!(output.contains("oxphp_workers_idle 8"));
    }

    #[test]
    fn test_workers_idle_computed_from_current_minus_busy() {
        let m = Metrics::new();
        m.set_workers_current(7);
        m.request_queued();
        m.request_queued();
        m.request_queued();

        let output = m.to_prometheus();
        assert!(output.contains("oxphp_workers_idle 4"));

        m.request_dequeued();
        m.request_dequeued();
        m.request_dequeued();

        let output = m.to_prometheus();
        assert!(output.contains("oxphp_workers_idle 7"));
    }

    #[test]
    fn test_to_prometheus() {
        let m = Metrics::new();
        m.record_request(&Method::GET);
        m.record_response(200, Duration::from_micros(1000), 50, 500);

        let output = m.to_prometheus();
        assert!(output.contains("oxphp_requests_total 1"));
        assert!(output.contains("oxphp_requests_by_method_total{method=\"GET\"} 1"));
        assert!(output.contains("oxphp_responses_by_status_total{status=\"2xx\"} 1"));
        assert!(output.contains("oxphp_request_bytes_total 50"));
        assert!(output.contains("oxphp_response_bytes_total 500"));
        // _count derived from responses_by_status_class (one 200 → count=1)
        assert!(output.contains("oxphp_request_duration_us_count 1"));
        // _sum reuses total_response_time_us
        assert!(output.contains("oxphp_request_duration_us_sum 1000"));
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
        wm.record_recycle(1); // scheduled
        wm.record_recycle(2); // max_memory
        wm.record_recycle(3); // error
        wm.record_recycle(0); // shutdown — not counted as reason

        assert_eq!(wm.recycles_total.load(Ordering::Relaxed), 4);
        assert_eq!(wm.recycles_scheduled.load(Ordering::Relaxed), 1);
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
        wm.recycles_scheduled.fetch_add(1, Ordering::Relaxed);
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
        assert!(output.contains("oxphp_worker_recycles_by_reason_total{reason=\"scheduled\"} 1"));
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

    // ── New metrics tests ──

    #[test]
    fn test_request_duration_histogram() {
        let m = Metrics::new();
        m.record_response(200, Duration::from_micros(50), 0, 0); // bucket[0] (le=100)
        m.record_response(200, Duration::from_micros(600), 0, 0); // bucket[2] (le=1000)
        m.record_response(200, Duration::from_micros(5_000_000), 0, 0); // bucket[12] (+Inf)

        assert_eq!(m.request_duration_buckets[0].load(Ordering::Relaxed), 1); // <=100
        assert_eq!(m.request_duration_buckets[2].load(Ordering::Relaxed), 1); // <=1000
        assert_eq!(m.request_duration_buckets[12].load(Ordering::Relaxed), 1); // +Inf
                                                                               // _count derived from responses_by_status_class sum (all 200 → class[1])
        assert_eq!(m.responses_by_status_class[1].load(Ordering::Relaxed), 3);
        // _sum reuses total_response_time_us
        assert_eq!(
            m.total_response_time_us.load(Ordering::Relaxed),
            50 + 600 + 5_000_000
        );

        let output = m.to_prometheus();
        assert!(output.contains("oxphp_request_duration_us_bucket{le=\"100\"} 1"));
        assert!(output.contains("oxphp_request_duration_us_bucket{le=\"1000\"} 2")); // cumulative
        assert!(output.contains("oxphp_request_duration_us_bucket{le=\"+Inf\"} 3"));
        assert!(output.contains("oxphp_request_duration_us_count 3"));
    }

    #[test]
    fn test_bytes_counters() {
        let m = Metrics::new();
        m.record_response(200, Duration::from_micros(100), 1024, 4096);
        m.record_response(200, Duration::from_micros(100), 512, 2048);

        assert_eq!(m.request_bytes_total.load(Ordering::Relaxed), 1536);
        assert_eq!(m.response_bytes_total.load(Ordering::Relaxed), 6144);

        let output = m.to_prometheus();
        assert!(output.contains("oxphp_request_bytes_total 1536"));
        assert!(output.contains("oxphp_response_bytes_total 6144"));
    }

    #[test]
    fn test_queue_wait_histogram() {
        let m = Metrics::new();
        m.record_queue_wait(30); // bucket[0] (le=50)
        m.record_queue_wait(200); // bucket[2] (le=250)
        m.record_queue_wait(100_000); // bucket[9] (+Inf)

        assert_eq!(m.queue_wait_buckets[0].load(Ordering::Relaxed), 1);
        assert_eq!(m.queue_wait_buckets[2].load(Ordering::Relaxed), 1);
        assert_eq!(m.queue_wait_buckets[9].load(Ordering::Relaxed), 1);
        assert_eq!(m.queue_wait_count.load(Ordering::Relaxed), 3);
        assert_eq!(
            m.queue_wait_sum_us.load(Ordering::Relaxed),
            30 + 200 + 100_000
        );

        let output = m.to_prometheus();
        assert!(output.contains("oxphp_queue_wait_us_bucket{le=\"50\"} 1"));
        assert!(output.contains("oxphp_queue_wait_us_bucket{le=\"+Inf\"} 3"));
        assert!(output.contains("oxphp_queue_wait_us_count 3"));
    }

    #[test]
    fn test_rate_limited_counter() {
        let m = Metrics::new();
        m.rate_limited();
        m.rate_limited();

        assert_eq!(m.rate_limited_total.load(Ordering::Relaxed), 2);
        let output = m.to_prometheus();
        assert!(output.contains("oxphp_rate_limited_total 2"));
    }

    #[test]
    fn test_static_cache_counters() {
        let m = Metrics::new();
        m.static_cache_hit();
        m.static_cache_hit();
        m.static_cache_miss();

        assert_eq!(m.static_cache_hits.load(Ordering::Relaxed), 2);
        assert_eq!(m.static_cache_misses.load(Ordering::Relaxed), 1);

        let output = m.to_prometheus();
        assert!(output.contains("oxphp_static_cache_hits_total 2"));
        assert!(output.contains("oxphp_static_cache_misses_total 1"));
    }

    #[test]
    fn test_compression_counters() {
        let m = Metrics::new();
        m.record_compression(500);
        m.record_compression(1200);

        assert_eq!(m.compressed_responses_total.load(Ordering::Relaxed), 2);
        assert_eq!(
            m.compression_bytes_saved_total.load(Ordering::Relaxed),
            1700
        );

        let output = m.to_prometheus();
        assert!(output.contains("oxphp_compressed_responses_total 2"));
        assert!(output.contains("oxphp_compression_bytes_saved_total 1700"));
    }

    #[test]
    fn test_async_metrics() {
        let m = Metrics::new();
        m.async_task_dispatched();
        m.async_task_dispatched();
        m.async_task_completed();
        m.async_task_failed();
        m.async_task_cancelled();
        m.async_task_rejected();

        let prom = m.to_prometheus();
        assert!(prom.contains("oxphp_async_tasks_dispatched_total 2"));
        assert!(prom.contains("oxphp_async_tasks_completed_total 1"));
        assert!(prom.contains("oxphp_async_tasks_failed_total 1"));
        assert!(prom.contains("oxphp_async_tasks_cancelled_total 1"));
        assert!(prom.contains("oxphp_async_tasks_rejected_total 1"));
    }
}
