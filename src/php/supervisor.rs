//! Per-second supervisor that scans WorkerHeartbeats and emits
//! observability metrics. No automatic intervention — D only sees;
//! operators react to the metrics.

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StuckKind {
    Io,
    CCall,
    Cpu,
}

impl StuckKind {
    pub fn label(&self) -> &'static str {
        match self {
            StuckKind::Io => "io",
            StuckKind::CCall => "c_call",
            StuckKind::Cpu => "cpu",
        }
    }
}

/// `cpu_delta == 0` → stuck on syscall/lock (`io`).
/// `cpu_delta > 0 && tick_delta == 0` → inside C code (`c_call`).
/// `cpu_delta > 0 && tick_delta > 0` → PHP loop with function calls (`cpu`).
pub fn classify(cpu_delta: u64, tick_delta: u64) -> StuckKind {
    if cpu_delta == 0 {
        StuckKind::Io
    } else if tick_delta == 0 {
        StuckKind::CCall
    } else {
        StuckKind::Cpu
    }
}

/// Reads cumulative thread CPU time in microseconds. Linux-only in
/// production; Darwin returns None and the supervisor falls back to
/// `kind=Io` classification (Darwin is dev-only — see Cargo features).
pub fn read_thread_cpu_us(tid: u64) -> Option<u64> {
    if tid == 0 {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        // Uncached fallback used only by tests / non-supervisor callers.
        // The supervisor's hot path uses `CpuStatCache` to keep per-slot
        // fds open across scans (see `Supervisor::scan_once_at`).
        read_thread_cpu_us_linux_uncached(tid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = tid;
        None
    }
}

#[cfg(target_os = "linux")]
fn parse_cpu_us_from_stat(buf: &str) -> Option<u64> {
    // Format: "<pid> (<comm>) <state> <ppid> ...". `<comm>` may
    // contain spaces and parens, so locate the rightmost ')'.
    let close = buf.rfind(')')?;
    // After `') '` come state, ppid, pgrp, session, tty_nr, tpgid,
    // flags, minflt, cminflt, majflt, cmajflt, utime, stime, ...
    let after = &buf[close + 2..];
    let mut fields = after.split_whitespace();
    for _ in 0..11 {
        fields.next()?;
    }
    let utime: u64 = fields.next()?.parse().ok()?;
    let stime: u64 = fields.next()?.parse().ok()?;
    let ticks_per_sec = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as u64;
    if ticks_per_sec == 0 {
        return None;
    }
    Some((utime + stime) * 1_000_000 / ticks_per_sec)
}

#[cfg(target_os = "linux")]
fn read_thread_cpu_us_linux_uncached(tid: u64) -> Option<u64> {
    use std::io::Read;
    let path = format!("/proc/self/task/{}/stat", tid);
    let mut buf = String::with_capacity(512);
    std::fs::File::open(&path)
        .ok()?
        .read_to_string(&mut buf)
        .ok()?;
    parse_cpu_us_from_stat(&buf)
}

/// Per-slot fd cache for `/proc/self/task/<tid>/stat`. The supervisor
/// thread is the only reader; one cache instance lives for the
/// supervisor's lifetime. Open is a syscall (~few μs); on a 1 Hz scan
/// across N stuck workers, the cache amortises that to one open per
/// `(slot, tid)` pair. When tid changes (worker respawn), the cached
/// fd is replaced.
#[cfg(target_os = "linux")]
struct CpuStatCache {
    slots: Vec<Option<(u64, std::fs::File)>>,
}

#[cfg(target_os = "linux")]
impl CpuStatCache {
    fn new() -> Self {
        Self { slots: Vec::new() }
    }

    fn read(&mut self, slot_id: usize, tid: u64) -> Option<u64> {
        if tid == 0 {
            return None;
        }
        if slot_id >= self.slots.len() {
            self.slots.resize_with(slot_id + 1, || None);
        }
        let entry = &mut self.slots[slot_id];
        if entry.as_ref().map(|(t, _)| *t) != Some(tid) {
            // Either uncached or tid changed (worker respawn).
            let path = format!("/proc/self/task/{}/stat", tid);
            match std::fs::File::open(&path) {
                Ok(f) => *entry = Some((tid, f)),
                Err(_) => {
                    *entry = None;
                    return None;
                }
            }
        }
        let (_, file) = entry.as_mut()?;
        // /proc files re-evaluate on every read; rewind to start before
        // reading. If seek/read fails, drop the cache so the next scan
        // re-opens.
        use std::io::{Read, Seek, SeekFrom};
        if file.seek(SeekFrom::Start(0)).is_err() {
            *entry = None;
            return None;
        }
        let mut buf = String::with_capacity(512);
        if file.read_to_string(&mut buf).is_err() {
            *entry = None;
            return None;
        }
        parse_cpu_us_from_stat(&buf)
    }
}

use std::sync::Arc;
use std::time::Duration;

use crate::metrics::Metrics;
use crate::php::heartbeat::monotonic_us;
use crate::php::worker_registry::{WorkerSlot, WORKERS};

const DEFAULT_SCAN_PERIOD: Duration = Duration::from_secs(1);
const DEFAULT_STUCK_THRESHOLD_US: u64 = 60 * 1_000_000;

pub struct Supervisor {
    pub scan_period: Duration,
    pub stuck_threshold_us: u64,
    pub metrics: Arc<Metrics>,
}

impl Supervisor {
    pub fn production(metrics: Arc<Metrics>) -> Self {
        Self {
            scan_period: DEFAULT_SCAN_PERIOD,
            stuck_threshold_us: DEFAULT_STUCK_THRESHOLD_US,
            metrics,
        }
    }

    /// Test-only constructor. Lets integration tests use a small
    /// threshold to exercise the stuck path without waiting 60 s.
    pub fn with_threshold(metrics: Arc<Metrics>, threshold_us: u64, period: Duration) -> Self {
        Self {
            scan_period: period,
            stuck_threshold_us: threshold_us,
            metrics,
        }
    }

    pub fn spawn(
        self,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
    ) -> std::thread::JoinHandle<()> {
        std::thread::Builder::new()
            .name("oxphp-supervisor".into())
            .spawn(move || self.run(shutdown))
            .expect("spawn oxphp-supervisor thread")
    }

    fn run(self, shutdown: Arc<std::sync::atomic::AtomicBool>) {
        #[cfg(target_os = "linux")]
        let mut cpu_cache = CpuStatCache::new();
        while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(self.scan_period);
            if let Some(workers) = WORKERS.get() {
                #[cfg(target_os = "linux")]
                self.scan_once_at_with_cache(workers, monotonic_us(), &mut cpu_cache);
                #[cfg(not(target_os = "linux"))]
                self.scan_once(workers);
            }
        }
    }

    pub fn scan_once(&self, workers: &[WorkerSlot]) {
        self.scan_once_at(workers, monotonic_us());
    }

    /// Test seam: same as `scan_once` but takes an explicit `now_us`
    /// so tests don't depend on the monotonic-clock warm-up state.
    /// Uses the uncached `read_thread_cpu_us` path; the supervisor's
    /// `run()` loop uses `scan_once_at_with_cache` instead.
    pub fn scan_once_at(&self, workers: &[WorkerSlot], now_us: u64) {
        for (id, slot) in workers.iter().enumerate() {
            self.scan_slot(id, slot, now_us, read_thread_cpu_us);
        }
    }

    #[cfg(target_os = "linux")]
    fn scan_once_at_with_cache(
        &self,
        workers: &[WorkerSlot],
        now_us: u64,
        cache: &mut CpuStatCache,
    ) {
        for (id, slot) in workers.iter().enumerate() {
            self.scan_slot(id, slot, now_us, |tid| cache.read(id, tid));
        }
    }

    fn scan_slot(
        &self,
        id: usize,
        slot: &WorkerSlot,
        now_us: u64,
        mut read_cpu_us: impl FnMut(u64) -> Option<u64>,
    ) {
        let start = slot
            .heartbeat
            .request_start_us
            .load(std::sync::atomic::Ordering::Relaxed);
        if start == 0 {
            self.metrics.observe_age(id, 0);
            return;
        }
        let age_us = now_us.saturating_sub(start);
        self.metrics.observe_age(id, age_us);

        if age_us < self.stuck_threshold_us {
            return;
        }
        self.metrics.observe_long_running(id);

        let tid = slot
            .heartbeat
            .tid
            .load(std::sync::atomic::Ordering::Relaxed);
        let cpu_us = read_cpu_us(tid).unwrap_or(0);
        let prev_cpu = slot
            .heartbeat
            .last_cpu_us
            .swap(cpu_us, std::sync::atomic::Ordering::Relaxed);
        let cpu_delta = cpu_us.saturating_sub(prev_cpu);

        let ticks = slot
            .heartbeat
            .ticks
            .load(std::sync::atomic::Ordering::Relaxed);
        let prev_ticks = slot
            .heartbeat
            .last_ticks
            .swap(ticks, std::sync::atomic::Ordering::Relaxed);
        let tick_delta = ticks.saturating_sub(prev_ticks);

        let kind = classify(cpu_delta, tick_delta);
        self.metrics.observe_stuck(id, kind);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_table() {
        assert_eq!(classify(0, 0), StuckKind::Io);
        assert_eq!(classify(0, 5), StuckKind::Io);
        assert_eq!(classify(10, 0), StuckKind::CCall);
        assert_eq!(classify(10, 5), StuckKind::Cpu);
    }

    #[test]
    fn label_round_trip() {
        assert_eq!(StuckKind::Io.label(), "io");
        assert_eq!(StuckKind::CCall.label(), "c_call");
        assert_eq!(StuckKind::Cpu.label(), "cpu");
    }

    #[test]
    fn read_thread_cpu_us_zero_tid_returns_none() {
        assert!(read_thread_cpu_us(0).is_none());
    }

    #[test]
    fn read_thread_cpu_us_unknown_tid_returns_none() {
        // tid 999_999_999 essentially never exists; on Linux we get
        // ENOENT, on Darwin we always return None. Either way: None.
        assert!(read_thread_cpu_us(999_999_999).is_none());
    }

    #[test]
    fn scan_once_writes_age_for_busy_slot_and_zero_for_idle() {
        use crate::metrics::Metrics;
        use crate::php::worker_registry::{init_workers, WORKERS};

        // OnceLock is a single global across the whole test process.
        // Tests pick a slot index of their own and reset it after, so
        // they're order-independent.
        // Use the same N as worker_registry::tests so whichever test
        // wins the OnceLock, the other still sees the expected size.
        init_workers(4);
        let workers = WORKERS.get().unwrap();
        let busy = 0usize;
        let idle = 1usize;

        const FAKE_NOW_US: u64 = 10_000_000;
        const AGE_US: u64 = 500_000;
        workers[busy]
            .heartbeat
            .request_start_us
            .store(FAKE_NOW_US - AGE_US, std::sync::atomic::Ordering::Relaxed);
        workers[idle]
            .heartbeat
            .request_start_us
            .store(0, std::sync::atomic::Ordering::Relaxed);

        let metrics = Arc::new(Metrics::new_with_workers(workers.len()));
        let s = Supervisor::with_threshold(metrics.clone(), 10_000_000, Duration::from_millis(1));
        s.scan_once_at(workers, FAKE_NOW_US);

        let busy_age =
            metrics.worker_request_age_us[busy].load(std::sync::atomic::Ordering::Relaxed);
        assert_eq!(busy_age, AGE_US);
        assert_eq!(
            metrics.worker_request_age_us[idle].load(std::sync::atomic::Ordering::Relaxed),
            0
        );
        assert_eq!(
            metrics.worker_long_running_total[busy].load(std::sync::atomic::Ordering::Relaxed),
            0,
            "below threshold should not bump long_running"
        );

        // Reset so other tests start clean.
        workers[busy]
            .heartbeat
            .request_start_us
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }

    #[test]
    fn scan_once_above_threshold_classifies_stuck() {
        use crate::metrics::Metrics;
        use crate::php::worker_registry::{init_workers, WORKERS};

        // Use the same N as worker_registry::tests so whichever test
        // wins the OnceLock, the other still sees the expected size.
        init_workers(4);
        let workers = WORKERS.get().unwrap();
        let id = 2usize;

        const FAKE_NOW_US: u64 = 10_000_000;
        const AGE_US: u64 = 2_000_000;
        workers[id]
            .heartbeat
            .request_start_us
            .store(FAKE_NOW_US - AGE_US, std::sync::atomic::Ordering::Relaxed);
        // tid=0 → read_thread_cpu_us returns None → cpu_delta=0 → classify==Io.
        workers[id]
            .heartbeat
            .tid
            .store(0, std::sync::atomic::Ordering::Relaxed);

        let metrics = Arc::new(Metrics::new_with_workers(workers.len()));
        let s = Supervisor::with_threshold(metrics.clone(), 500_000, Duration::from_millis(1));
        s.scan_once_at(workers, FAKE_NOW_US);

        assert_eq!(
            metrics.worker_long_running_total[id].load(std::sync::atomic::Ordering::Relaxed),
            1
        );
        assert_eq!(
            metrics.worker_stuck_total_io[id].load(std::sync::atomic::Ordering::Relaxed),
            1
        );

        workers[id]
            .heartbeat
            .request_start_us
            .store(0, std::sync::atomic::Ordering::Relaxed);
    }
}
