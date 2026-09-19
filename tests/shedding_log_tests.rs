//! What the log says while the server is shedding load.
//!
//! An instance refusing a fifth of its traffic for overload wrote nothing at
//! all: the refusals were counted, and `oxphp_admission_refused_total` is what
//! an alert reads, but the log of an overloaded server and the log of an idle
//! one were the same log. Whoever went looking during the incident — which is
//! when a log is read — found no mention of it.
//!
//! A test of its own binary, and deliberately so. `tracing` decides once per
//! call site whether anybody wants its events and caches the answer for the
//! whole process. The answer is recomputed whenever a subscriber is created,
//! and while at most one is registered it is computed from what the thread
//! doing the rebuilding has installed — so in a shared test binary a thread
//! with no subscriber of its own can cache "nobody wants this" onto a call
//! site another thread is at that moment trying to capture, and can cache it
//! again each time another sibling installs a subscriber of its own (dropping
//! one recomputes nothing). Not a hypothetical: the unit-level
//! version of this test captured both lines when run alone and captured
//! nothing at all beside its siblings. A global subscriber is the answer for
//! every thread at once and may be installed once per process, so this test
//! is given a process to itself and installs it before anything has logged.
//!
//! The supervisor runs here for real, as its own thread on its own clock, so
//! this also pins the part no unit test can see: that the scan loop asks the
//! shedding watch at all.

use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use oxphp::executor::admission::ShedReason;
use oxphp::metrics::{Metrics, QueueSnapshot};
use oxphp::php::supervisor::Supervisor;

/// A scan period short enough that the minute-long thresholds are milliseconds
/// of wall clock, and long enough that a loaded machine still gets through the
/// scans inside the poll below.
const SCAN_PERIOD: Duration = Duration::from_millis(5);

/// How long a line is waited for before the test gives up on it.
const PATIENCE: Duration = Duration::from_secs(20);

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Captured {
    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).expect("utf-8 log")
    }

    /// Wait until the log contains `needle`, or fail with everything written.
    fn wait_for(&self, needle: &str) -> String {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let text = self.text();
            if text.contains(needle) {
                return text;
            }
            assert!(
                Instant::now() < deadline,
                "waited {PATIENCE:?} for {needle:?} and the log holds: {text}"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

/// The numeric value logged under `name`, from the last line that carries it.
///
/// Deliberately a string scan rather than a JSON parse: what is being asserted
/// is the shape an alert rule matches on, and a parser that accepts `"4812"`
/// as well as `4812` would accept exactly the rendering this server must not
/// emit.
fn field(log: &str, name: &str) -> u64 {
    let key = format!("\"{name}\":");
    let tail = log
        .rsplit_once(&key)
        .unwrap_or_else(|| panic!("no {name} field in the log: {log}"))
        .1;
    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
    digits
        .parse()
        .unwrap_or_else(|_| panic!("{name} is not a JSON number in: {log}"))
}

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Captured {
    type Writer = Captured;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// The pool the report was measured on: seven workers behind a queue holding
/// 896 with no admission slot free, and the wait budget already driven down
/// from its ceiling.
///
/// Every number here is distinct from every other, the capacity deliberately
/// wider than the depth. Equal figures would let the line map any of these
/// fields onto any other and still read correctly.
///
/// Deliberately without worker metrics. That makes it a traditional-mode pool,
/// where the stall watch has no progress counter to judge and says nothing —
/// so every line captured below came from the shedding report, and the report
/// is shown to work in the mode the measurement was taken in, which is also
/// the default one.
fn shedding_metrics() -> Arc<Metrics> {
    let metrics = Arc::new(Metrics::new_with_workers(7));
    metrics.set_queue_probe(Box::new(|| QueueSnapshot {
        depth: 896,
        capacity: 1_024,
        slots_available: 0,
        wait_budget_us: 15_625,
        wait_budget_ceiling_us: 1_000_000,
    }));
    metrics.set_workers_current(7);
    metrics
}

#[test]
fn a_shedding_episode_starts_and_ends_in_the_log() {
    let captured = Captured::default();
    let subscriber = tracing_subscriber::fmt()
        .json()
        .with_writer(captured.clone())
        .with_max_level(tracing::Level::TRACE)
        .finish();
    tracing::subscriber::set_global_default(subscriber).expect("first subscriber in this process");

    let metrics = shedding_metrics();
    let shutdown = Arc::new(AtomicBool::new(false));
    let supervisor = Supervisor::with_threshold(Arc::clone(&metrics), 60_000_000, SCAN_PERIOD);
    let handle = supervisor.spawn(Arc::clone(&shutdown));

    // No wait for the supervisor to settle first: the watch has no seeding
    // scan to get out of the way, so refusals counted before its first fold
    // are reported by that fold rather than swallowed by it. Whether the scan
    // boundary falls before this loop, inside it or after it changes only how
    // the count is split between the opening line and the episode total.

    // 4812 requests answered 529 in one go, which is the traffic the report
    // describes arriving faster than seven workers can take it.
    for _ in 0..4_812 {
        metrics.request_admission_refused(ShedReason::WaitTimeout);
    }

    let start = captured.wait_for("has started shedding requests");
    // `refused` is one scan's worth, and the scan boundary falls where it
    // falls: a supervisor that wakes in the middle of the loop above opens the
    // episode on the part it can see and folds the rest into the next scan.
    // Pinning the whole 4812 here would be pinning that the boundary missed,
    // which it does not always — what the line has to carry is a count of this
    // scan's refusals, and the episode's own total is asserted on the closing
    // line, where it is not a race.
    let refused = field(&start, "refused");
    assert!(
        (1..=4_812).contains(&refused),
        "the opening line has to say how much is being shed: {start}"
    );
    assert!(
        start.contains("\"queue_depth\":896")
            && start.contains("\"queue_capacity\":1024")
            && start.contains("\"admission_slots_available\":0"),
        "and where the capacity went: {start}"
    );
    assert!(
        start.contains("\"wait_budget_us\":15625")
            && start.contains("\"wait_budget_ceiling_us\":1000000"),
        "and the budget beside the ceiling it was configured with, or the two \
         cases the pair separates cannot be told apart: {start}"
    );

    // Nothing more is refused, so the episode runs out its quiet minute — 60
    // scans — and closes itself.
    let stop = captured.wait_for("has stopped shedding requests");
    assert!(
        stop.contains("\"episode_refused\":4812"),
        "the closing line has to carry what the episode cost: {stop}"
    );

    // One line each way, not one per refused request: at these rates a line
    // per refusal is itself an outage.
    assert_eq!(
        stop.matches("has started shedding requests").count(),
        1,
        "one opening line for the episode: {stop}"
    );
    assert_eq!(
        stop.matches("has stopped shedding requests").count(),
        1,
        "one closing line for the episode: {stop}"
    );

    shutdown.store(true, Ordering::Relaxed);
    handle.join().expect("supervisor thread");
}
