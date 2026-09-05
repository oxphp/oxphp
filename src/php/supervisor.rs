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

/// Scans a stall keeps quiet between repeats. At the default one-second scan
/// period that is one line a minute — often enough that a wedge which started
/// hours ago is still saying so in the last page of the log, rare enough that
/// it does not bury the traffic around it.
///
/// It is also how long readiness waits before reporting the wedge, since the
/// flag goes up on the first repeat rather than on the first confirmation.
/// Retuning this for log noise retunes that too, and the delay is quoted as
/// "a minute" in the metric's HELP text and in the operations documentation.
const STALL_REPEAT_SCANS: u64 = 60;

/// Consecutive stalled scans before the state is announced.
///
/// One is not enough. A request occupies the queue for the microseconds
/// between admission and pickup, so a scan can land on a queue of one with no
/// worker yet marked busy and nothing completed in that second, on a server
/// that is perfectly healthy and merely quiet. A wedge, by contrast, is still
/// there on the next scan and every scan after it, so a second look costs one
/// second of delay and removes the whole class of sampling artefacts.
const STALL_CONFIRM_SCANS: u64 = 2;

/// One scan's reading of the pool, as [`StallWatch`] wants it.
///
/// A struct rather than four positional arguments because two of them are
/// counts of requests and two are counts of workers, and at a call site the
/// compiler would not notice them being swapped.
#[derive(Copy, Clone, Debug)]
pub struct PoolScan {
    /// Requests admitted to the queue and not yet picked up by a worker.
    pub queue_depth: usize,
    /// Cumulative overload refusals.
    pub refused_total: u64,
    /// Cumulative work the pool has got through — see
    /// [`Metrics::pool_progress_total`](crate::metrics::Metrics::pool_progress_total).
    pub progress_total: u64,
    /// Worker threads with nothing in flight.
    pub idle_workers: usize,
}

/// What the admission-stall watch has to say about the scan just taken.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StallTransition {
    /// Nothing worth a line.
    Quiet,
    /// The pool has just started refusing work it has workers for.
    Entered,
    /// It is still doing that, and enough scans have passed to say so again.
    Persisting,
    /// It served a request again.
    Recovered,
}

/// Watches for the one state the pool's own metrics each read as healthy: work
/// waiting for a worker while every worker sits idle and nothing completes.
///
/// Each signal is ordinary on its own — a queue has requests in it, workers
/// have headroom, no completions means no traffic — and no existing series
/// pairs them, so the contradiction had to be assembled by hand from a scrape.
/// Held together it is not a load level but a fault: work is waiting, the pool
/// has room, and none of it is getting through.
///
/// Waiting work is counted two ways because the fault presents in two phases.
/// It starts as a queue that nobody is draining, which refuses nothing at all —
/// arrivals are still admitted, they simply pile up. Only once the queue is
/// full does admission start refusing, which is the phase an operator is most
/// likely to see and the one that has a counter. Watching refusals alone would
/// stay silent through the whole first phase, and on a server with modest
/// traffic that phase can last for hours.
///
/// Deliberately a pure state machine over totals: no clock, no logging, no
/// registry. The caller supplies the reading, which is what makes the rule
/// testable without a wedged server to hand.
#[derive(Default)]
pub struct StallWatch {
    /// The first scan has no window behind it — its deltas would be the
    /// server's whole history.
    seeded: bool,
    prev_refused: u64,
    prev_progress: u64,
    /// Scans spent in the stalled state. Cleared by the pool making progress,
    /// and by a non-stalled scan while still below the confirmation threshold;
    /// a lull after the state was announced is neither progress nor a stall
    /// and leaves it standing, which is what keeps a wedged pool with no
    /// traffic on it from reading as recovered.
    stalled_scans: u64,
}

impl StallWatch {
    /// Fold one scan into the watch.
    pub fn observe(&mut self, scan: PoolScan) -> StallTransition {
        let refused_delta = scan.refused_total.saturating_sub(self.prev_refused);
        let progress_delta = scan.progress_total.saturating_sub(self.prev_progress);
        self.prev_refused = scan.refused_total;
        self.prev_progress = scan.progress_total;

        if !self.seeded {
            self.seeded = true;
            return StallTransition::Quiet;
        }

        // A pool that has never got through anything has not shown it can, and
        // the shape of one still booting is the shape of one wedged: worker
        // mode publishes its worker count before any worker has finished
        // running the application's bootstrap, so a pod taking traffic during
        // a boot that runs into seconds has a filling queue, workers that read
        // idle because none has begun a request, and no progress. Warning on
        // every cold start is how an operator learns to ignore this line.
        if scan.progress_total == 0 {
            return StallTransition::Quiet;
        }

        let work_waiting = scan.queue_depth > 0 || refused_delta > 0;
        if work_waiting && progress_delta == 0 && scan.idle_workers > 0 {
            self.stalled_scans += 1;
            return match self.stalled_scans {
                n if n < STALL_CONFIRM_SCANS => StallTransition::Quiet,
                n if n == STALL_CONFIRM_SCANS => StallTransition::Entered,
                n if (n - STALL_CONFIRM_SCANS).is_multiple_of(STALL_REPEAT_SCANS) => {
                    StallTransition::Persisting
                }
                _ => StallTransition::Quiet,
            };
        }

        // Below the confirmation threshold nothing was ever said, so there is
        // nothing to take back: drop the count and stay quiet.
        if self.stalled_scans < STALL_CONFIRM_SCANS {
            self.stalled_scans = 0;
            return StallTransition::Quiet;
        }

        // Past it, leaving the state takes a worker actually getting through a
        // request rather than merely a scan with nothing waiting. A wedged pool with no
        // traffic on it has an empty queue and refuses nothing, so treating
        // quiet as recovery would clear the state on the first lull and
        // re-announce it on the next arrival — reporting the traffic pattern
        // rather than the fault.
        if progress_delta > 0 {
            self.stalled_scans = 0;
            return StallTransition::Recovered;
        }

        StallTransition::Quiet
    }
}

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
        let mut stall = StallWatch::default();
        while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(self.scan_period);
            if let Some(workers) = WORKERS.get() {
                #[cfg(target_os = "linux")]
                self.scan_once_at_with_cache(workers, monotonic_us(), &mut cpu_cache);
                #[cfg(not(target_os = "linux"))]
                self.scan_once(workers);
            }
            let _ = self.report_admission_stall(&mut stall);
        }
    }

    /// Say out loud when the pool leaves work waiting that it has workers for.
    ///
    /// The state is derivable from series that already exist, and that is
    /// exactly the problem: an operator has to think to pair them, and the
    /// pairing only occurs to someone who already suspects the fault. A pool
    /// wedged this way answers `200` on liveness for good and on readiness
    /// for the first minute — readiness turns only once the state has
    /// persisted that long — and serves static files normally, so this line is
    /// the first thing that will say anything is wrong.
    ///
    /// The numbers on it are what makes the line diagnostic rather than merely
    /// alarming: `queue_depth` short of `queue_capacity` with slots still free
    /// is a queue nobody is consuming and arrivals still being admitted;
    /// `queue_depth` at capacity with no slots free is the same fault once the
    /// queue has filled and admission has started refusing; and `queue_depth`
    /// at zero with no slots free is permits taken and never returned.
    ///
    /// Returns what it decided, so a test can pin the wiring between the
    /// metrics and the rule — which numbers reach which field is the one part
    /// of this the rule's own tests cannot see.
    fn report_admission_stall(&self, watch: &mut StallWatch) -> StallTransition {
        self.report_admission_stall_with(watch, crate::php::worker_registry::busy_workers())
    }

    /// Test seam: the same check against an explicit busy count.
    ///
    /// `busy_workers()` reads a process-global registry that other tests in
    /// the same binary mark up and clear again, so a test that took it from
    /// there would pass or fail on what happened to be running beside it.
    /// Passing it in also pins the last piece of the wiring: that the busy
    /// count is what decides whether the pool has headroom.
    fn report_admission_stall_with(
        &self,
        watch: &mut StallWatch,
        busy_workers: usize,
    ) -> StallTransition {
        // An executor without a queue has no admission to stall.
        let Some(queue) = self.metrics.queue_snapshot() else {
            return StallTransition::Quiet;
        };
        // And a pool that does not count what its workers take off the queue
        // cannot be judged: the only number left would be completions, which
        // a storm of client aborts drives to zero on a pool working flat out.
        // Saying nothing is right rather than merely safe — the state this
        // watches for is a worker parked with a request it will never run,
        // which is not reachable where a worker blocks on the queue itself.
        let Some(progress_total) = self.metrics.pool_progress_total() else {
            return StallTransition::Quiet;
        };
        let workers_idle = self.metrics.workers_current().saturating_sub(busy_workers);

        let transition = watch.observe(PoolScan {
            queue_depth: queue.depth,
            refused_total: self.metrics.admission_refused_overload_total(),
            progress_total,
            idle_workers: workers_idle,
        });
        // Published as well as logged: the readiness probe cannot read the
        // log, and it is the only reader that can act on this state on its
        // own. `Quiet` deliberately leaves the flag where it is — the watch
        // holds the state through a lull, and so must the instance's place in
        // rotation.
        //
        // The flag waits for the repeat rather than going up with the first
        // warning. Two scans are enough to say something on a line an
        // operator reads with the rest of the context; they are not enough to
        // take an instance out of rotation, because a worker that exited on
        // its memory ceiling and is re-running the application's bootstrap
        // presents exactly this shape — the pool still counts it, it has
        // begun no request so it reads idle, the queue behind it fills, and
        // nothing completes. That is a healthy pool a second or two from
        // serving again, and removing it from rotation takes away the very
        // traffic whose completion is the only thing that clears the state.
        // A bootstrap that is still running a minute later is not that.
        match transition {
            StallTransition::Entered | StallTransition::Persisting => {
                if transition == StallTransition::Persisting {
                    self.metrics.set_pool_stalled(true);
                }
                tracing::warn!(
                    queue_depth = queue.depth,
                    queue_capacity = queue.capacity,
                    admission_slots_available = queue.slots_available,
                    workers_idle,
                    "PHP requests are waiting while the pool has idle workers and got nothing done since the last scan"
                );
            }
            StallTransition::Recovered => {
                self.metrics.set_pool_stalled(false);
                tracing::info!(
                    queue_depth = queue.depth,
                    admission_slots_available = queue.slots_available,
                    workers_idle,
                    "PHP pool is reaching workers again"
                );
            }
            StallTransition::Quiet => {}
        }
        transition
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

    /// Where the reproduction's progress counter froze: the pool had handled
    /// 176 438 requests and then stopped, which is the shape the fault takes —
    /// a pool that has been working, not one that never started.
    const HANDLED_BEFORE_WEDGE: u64 = 176_438;

    /// A reading in which the pool is doing nothing with work it has, with its
    /// progress counter frozen where the wedge left it.
    fn wedged_scan(queue_depth: usize, refused: u64, progress_after: u64) -> PoolScan {
        let progress = HANDLED_BEFORE_WEDGE + progress_after;
        PoolScan {
            queue_depth,
            refused_total: refused,
            progress_total: progress,
            idle_workers: 4,
        }
    }

    /// Metrics wired up the way a wedged server presents: `depth` requests
    /// queued in a 512-deep queue with the rest of the permits free, four
    /// workers, none busy, and a progress counter that stands still.
    fn wedged_supervisor(depth: usize) -> (Arc<Metrics>, Supervisor) {
        let metrics = Arc::new(Metrics::new_with_workers(4));
        metrics.set_queue_probe(Box::new(move || crate::metrics::QueueSnapshot {
            depth,
            capacity: 512,
            slots_available: 512 - depth,
        }));
        metrics.set_workers_current(4);
        // Worker mode is the only pool that counts what it takes off the
        // queue, and the watch refuses to judge one that does not.
        let wm = Arc::new(crate::metrics::WorkerMetrics::new(4));
        wm.requests_handled_total
            .fetch_add(176_438, std::sync::atomic::Ordering::Relaxed);
        metrics.set_worker_metrics(Arc::clone(&wm));
        let supervisor =
            Supervisor::with_threshold(Arc::clone(&metrics), 500_000, Duration::from_millis(1));
        (metrics, supervisor)
    }

    #[test]
    fn the_supervisor_reports_the_state_measured_on_a_wedged_server() {
        // The numbers are the ones a live wedged server was caught with: 45
        // requests queued in a 512-deep queue, 467 admission slots still free,
        // four workers, none busy, nothing refused, and a progress counter
        // frozen where it stopped.
        let (_metrics, supervisor) = wedged_supervisor(45);
        let mut watch = StallWatch::default();
        assert_eq!(
            supervisor.report_admission_stall_with(&mut watch, 0),
            StallTransition::Quiet,
            "first scan only seeds the window"
        );
        assert_eq!(
            supervisor.report_admission_stall_with(&mut watch, 0),
            StallTransition::Quiet,
            "one scan is not enough to announce"
        );
        assert_eq!(
            supervisor.report_admission_stall_with(&mut watch, 0),
            StallTransition::Entered
        );
    }

    #[test]
    fn the_supervisor_reads_the_queue_depth_and_not_a_neighbouring_number() {
        // The rule's own tests cannot see which number reaches which field, and
        // every number on a wedged server's probe is non-zero, so a wiring that
        // passed `capacity` or `slots_available` as the depth would satisfy them
        // all. An empty queue is where the three come apart: `capacity` and
        // `slots_available` are both 512 there and would keep announcing a
        // wedge on an idle server for ever, while the depth is 0 and nothing is
        // waiting.
        let (_metrics, supervisor) = wedged_supervisor(0);
        let mut watch = StallWatch::default();
        for scan in 0..5 {
            assert_eq!(
                supervisor.report_admission_stall_with(&mut watch, 0),
                StallTransition::Quiet,
                "scan {scan}: an empty queue with nothing refused is an idle server"
            );
        }
    }

    #[test]
    fn the_supervisor_reads_the_busy_count_as_the_pool_s_headroom() {
        // The third field of the wiring. A pool whose every worker is busy is
        // at capacity, not wedged, however deep the queue behind it — and a
        // busy count that never reached the rule would turn every overloaded
        // server into a reported fault.
        let (_metrics, supervisor) = wedged_supervisor(45);
        let mut watch = StallWatch::default();
        for scan in 0..5 {
            assert_eq!(
                supervisor.report_admission_stall_with(&mut watch, 4),
                StallTransition::Quiet,
                "scan {scan}: all four workers busy is a pool at capacity"
            );
        }
    }

    #[test]
    fn the_supervisor_clears_the_state_when_the_pool_starts_working() {
        // Pins the other half of the wiring: the progress counter has to be
        // the one that moves. A field wired to something that never changes
        // would leave the pool announced as wedged for the life of the process.
        let (metrics, supervisor) = wedged_supervisor(45);
        let mut watch = StallWatch::default();
        supervisor.report_admission_stall_with(&mut watch, 0);
        supervisor.report_admission_stall_with(&mut watch, 0);
        assert_eq!(
            supervisor.report_admission_stall_with(&mut watch, 0),
            StallTransition::Entered
        );

        metrics
            .worker_metrics()
            .expect("worker metrics were published above")
            .requests_handled_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            supervisor.report_admission_stall_with(&mut watch, 0),
            StallTransition::Recovered
        );
    }

    #[test]
    fn the_supervisor_publishes_the_wedge_for_the_readiness_probe() {
        // The log line reaches an operator reading logs. A probe cannot read
        // logs, and the internal listener answers it without going through
        // admission or the connection budget — which is why a wedged instance
        // stayed in rotation while it served nothing. The flag is the only
        // thing that carries the state out of this thread.
        let (metrics, supervisor) = wedged_supervisor(45);
        let mut watch = StallWatch::default();
        assert!(!metrics.pool_stalled(), "nothing seen yet");

        supervisor.report_admission_stall_with(&mut watch, 0);
        supervisor.report_admission_stall_with(&mut watch, 0);
        assert!(
            !metrics.pool_stalled(),
            "a single unconfirmed scan must not take an instance out of rotation"
        );

        assert_eq!(
            supervisor.report_admission_stall_with(&mut watch, 0),
            StallTransition::Entered
        );
        assert!(
            !metrics.pool_stalled(),
            "the first confirmation is worth a warning, not a removal from rotation: \
             a worker re-running the application's bootstrap after a recycle reads \
             exactly like this and is seconds from serving"
        );

        // Still there a minute later, which a bootstrap is not.
        let mut transition = StallTransition::Quiet;
        for _ in 0..STALL_REPEAT_SCANS {
            transition = supervisor.report_admission_stall_with(&mut watch, 0);
        }
        assert_eq!(transition, StallTransition::Persisting);
        assert!(
            metrics.pool_stalled(),
            "the wedge has to be readable from outside the supervisor thread"
        );

        // A lull is neither progress nor a stall: the watch holds the state,
        // and so must the flag — or a wedged pool with no traffic on it would
        // read as recovered and go straight back into rotation.
        assert_eq!(
            supervisor.report_admission_stall_with(&mut watch, 0),
            StallTransition::Quiet
        );
        assert!(metrics.pool_stalled(), "quiet is not recovery");

        metrics
            .worker_metrics()
            .expect("worker metrics were published above")
            .requests_handled_total
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        assert_eq!(
            supervisor.report_admission_stall_with(&mut watch, 0),
            StallTransition::Recovered
        );
        assert!(
            !metrics.pool_stalled(),
            "a pool serving again must come back into rotation without a restart"
        );
    }

    #[test]
    fn the_supervisor_says_nothing_for_a_pool_that_counts_no_progress() {
        // Outside worker mode nothing counts what the workers took off the
        // queue, and the only number left — completions — goes to zero under a
        // storm of client aborts on a pool working flat out. That is the shape
        // of the fault, on a healthy server, in the exact traffic that precedes
        // the real one.
        let metrics = Arc::new(Metrics::new_with_workers(4));
        metrics.set_queue_probe(Box::new(|| crate::metrics::QueueSnapshot {
            depth: 45,
            capacity: 512,
            slots_available: 467,
        }));
        metrics.set_workers_current(4);
        metrics.record_queue_wait(120);
        let supervisor = Supervisor::with_threshold(metrics, 500_000, Duration::from_millis(1));
        let mut watch = StallWatch::default();
        for _ in 0..5 {
            assert_eq!(
                supervisor.report_admission_stall_with(&mut watch, 0),
                StallTransition::Quiet
            );
        }
    }

    #[test]
    fn the_supervisor_says_nothing_without_a_queue_to_watch() {
        // The stub executor has no queue. Reading zeros off an absent probe
        // would make every benchmark run report a wedged pool.
        let metrics = Arc::new(Metrics::new_with_workers(4));
        metrics.set_workers_current(4);
        let supervisor = Supervisor::with_threshold(metrics, 500_000, Duration::from_millis(1));
        let mut watch = StallWatch::default();
        for _ in 0..5 {
            assert_eq!(
                supervisor.report_admission_stall_with(&mut watch, 0),
                StallTransition::Quiet
            );
        }
    }

    #[test]
    fn a_pool_that_has_never_served_is_booting_not_wedged() {
        // Worker mode boots the application once per worker, and the pool
        // publishes its worker count before any of them has finished doing so.
        // A pod taking traffic during a boot that runs into seconds therefore
        // has a filling queue, workers that read idle because none has begun a
        // request, and no progress — the wedge's exact shape, on a server that
        // is merely starting. A warning on every cold start is how an operator
        // learns to ignore this one.
        let booting = PoolScan {
            queue_depth: 120,
            refused_total: 0,
            progress_total: 0,
            idle_workers: 4,
        };
        let mut watch = StallWatch::default();
        for _ in 0..10 {
            assert_eq!(watch.observe(booting), StallTransition::Quiet);
        }
        // Once the pool has got through something it has shown it can, and the
        // ordinary rule applies from there.
        watch.observe(wedged_scan(120, 0, 0));
        watch.observe(wedged_scan(120, 0, 0));
        assert_eq!(
            watch.observe(wedged_scan(120, 0, 0)),
            StallTransition::Entered
        );
    }

    #[test]
    fn a_worker_churning_through_aborted_requests_is_not_a_stall() {
        // Under a storm of client aborts the client is gone before any
        // completion can be recorded against a connection, while the workers'
        // own count still sees every one of those requests end. Judged by
        // completions alone a pool handling thousands of requests a second
        // would read as stalled — and it would read that way in exactly the
        // traffic that precedes the real fault, which is where the two have to
        // stay distinguishable.
        let mut watch = StallWatch::default();
        let mut refused = 0;
        let mut handled = HANDLED_BEFORE_WEDGE;
        let mut storm_scan = || {
            refused += 40;
            handled += 5_000;
            PoolScan {
                queue_depth: 30,
                refused_total: refused,
                progress_total: handled,
                idle_workers: 4,
            }
        };
        watch.observe(storm_scan());
        watch.observe(storm_scan());
        assert_eq!(watch.observe(storm_scan()), StallTransition::Quiet);
    }

    #[test]
    fn stall_reported_while_the_queue_still_has_room() {
        // Measured, not imagined. A wedged pool was caught with 45 requests in
        // a 512-deep queue, four idle workers, and every refusal counter at
        // zero: the queue was nowhere near full, so nothing had been refused
        // and there was nothing to alert on. Watching refusals alone stays
        // silent through this whole first phase of the fault.
        let mut watch = StallWatch::default();
        watch.observe(wedged_scan(0, 0, 0));
        watch.observe(wedged_scan(45, 0, 0));
        assert_eq!(
            watch.observe(wedged_scan(45, 0, 0)),
            StallTransition::Entered
        );
    }

    #[test]
    fn stall_reported_when_idle_workers_refuse_everything() {
        // The later phase, and the one the issue reported: the queue has
        // filled, so arrivals are refused at admission rather than piling up.
        // Same fault, different counter.
        let mut watch = StallWatch::default();
        watch.observe(wedged_scan(0, 0, 0));
        watch.observe(wedged_scan(512, 12, 0));
        assert_eq!(
            watch.observe(wedged_scan(512, 24, 0)),
            StallTransition::Entered
        );
    }

    #[test]
    fn a_single_scan_is_not_enough_to_announce_a_stall() {
        // A request occupies the queue for the microseconds between admission
        // and pickup, so a scan can land on a queue of one with no worker yet
        // busy and nothing completed that second, on a healthy quiet server.
        // Announcing a catastrophic fault on that sample would cry wolf.
        let mut watch = StallWatch::default();
        watch.observe(wedged_scan(0, 0, 0));
        assert_eq!(watch.observe(wedged_scan(1, 0, 0)), StallTransition::Quiet);
        assert_eq!(
            watch.observe(wedged_scan(0, 0, 1)),
            StallTransition::Quiet,
            "picked up and served — nothing was announced, so there is nothing to recover from"
        );
    }

    #[test]
    fn first_scan_seeds_instead_of_reporting() {
        // A supervisor starting against a server that has been up for hours
        // would otherwise read its whole history as one scan's delta.
        let mut watch = StallWatch::default();
        assert_eq!(
            watch.observe(wedged_scan(0, 9_000, 0)),
            StallTransition::Quiet
        );
    }

    #[test]
    fn honest_overload_is_not_a_stall() {
        // A deep queue and climbing refusals while the pool serves is what a
        // bounded queue is for. Reporting it would make the warning worthless
        // exactly where 529s are expected.
        let mut watch = StallWatch::default();
        watch.observe(wedged_scan(0, 0, 0));
        watch.observe(wedged_scan(512, 500, 900));
        assert_eq!(
            watch.observe(wedged_scan(512, 1_000, 1_800)),
            StallTransition::Quiet
        );
    }

    #[test]
    fn a_pool_with_no_free_worker_is_not_a_stall() {
        // Every worker busy with a request that outlasts the scan, and a queue
        // behind them, is a pool at capacity. The contradiction is work waiting
        // *with* headroom.
        let mut watch = StallWatch::default();
        let busy = |queue_depth| PoolScan {
            queue_depth,
            refused_total: 500,
            progress_total: 1,
            idle_workers: 0,
        };
        watch.observe(busy(0));
        watch.observe(busy(512));
        assert_eq!(watch.observe(busy(512)), StallTransition::Quiet);
    }

    #[test]
    fn stall_repeats_on_a_bounded_cadence() {
        // A single line at the moment it broke is gone from the log tail by
        // the time anyone looks; a line per scan buries everything else.
        let mut watch = StallWatch::default();
        watch.observe(wedged_scan(0, 0, 0));
        let mut refused = 0;
        let seen: Vec<_> = (0..STALL_REPEAT_SCANS * 2 + STALL_CONFIRM_SCANS)
            .map(|_| {
                refused += 12;
                watch.observe(wedged_scan(45, refused, 0))
            })
            .collect();
        let announced = seen.iter().position(|t| *t == StallTransition::Entered);
        assert_eq!(
            announced,
            Some(STALL_CONFIRM_SCANS as usize - 1),
            "{seen:?}"
        );
        assert_eq!(
            seen.iter()
                .filter(|t| **t == StallTransition::Persisting)
                .count(),
            2,
            "{seen:?}"
        );
    }

    #[test]
    fn recovery_needs_the_pool_to_serve_again() {
        // A lull is not a recovery: a wedged pool with no traffic on it has an
        // empty queue and refuses nothing, so clearing the state on quiet would
        // re-announce the fault on every arrival — reporting the traffic
        // pattern rather than the fault. Only a request coming back from a
        // worker proves the pool is serving.
        let mut watch = StallWatch::default();
        watch.observe(wedged_scan(0, 0, 0));
        watch.observe(wedged_scan(45, 12, 0));
        assert_eq!(
            watch.observe(wedged_scan(45, 24, 0)),
            StallTransition::Entered
        );
        assert_eq!(
            watch.observe(wedged_scan(0, 24, 0)),
            StallTransition::Quiet,
            "no traffic at all — still wedged, not recovered"
        );
        assert_eq!(
            watch.observe(wedged_scan(45, 36, 0)),
            StallTransition::Quiet,
            "still stalled, not yet due a repeat"
        );
        assert_eq!(
            watch.observe(wedged_scan(0, 36, 1)),
            StallTransition::Recovered
        );
    }
}
