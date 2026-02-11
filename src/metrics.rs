use std::fmt::Write;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use http::Method;

/// Lock-free atomic metrics counters for the server.
/// All operations use `Relaxed` ordering — counters are approximate and don't
/// need happens-before guarantees with other data.
pub struct Metrics {
    start_time: Instant,
    total_requests: AtomicU64,
    active_connections: AtomicUsize,
    pending_requests: AtomicUsize,
    dropped_requests: AtomicU64,
    requests_by_method: [AtomicU64; 9],
    responses_by_status_class: [AtomicU64; 5],
    total_response_time_us: AtomicU64,
    busy_workers: AtomicUsize,
}

const METHOD_LABELS: [&str; 9] = [
    "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "CONNECT", "OTHER",
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
        _ => 8,
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
        assert_eq!(method_index(&Method::TRACE), 8);
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
}
