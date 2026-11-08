//! OPcache restarts in a server whose PHP threads are all one process.
//!
//! OPcache runs a scheduled restart in the startup of the next request once
//! no process holds a read lock on byte 1 of its lock file, and asks with
//! `fcntl(F_GETLK)`, which never reports the asking process's own locks. Every
//! PHP thread of the server is one process, so left alone the next request
//! would reset the shared memory under threads still running code from it.
//!
//! The server holds that byte itself with an open file description lock,
//! which `F_GETLK` does report to its own process, and lets go of it only
//! around the startup of the request picked to run the restart. That request
//! starts only once nothing runs code from the shared memory: each request
//! opened with the cache, each `oxphp_async()` task whose code is there and
//! each request that has run such code for another one (see [`adopt`]) counts
//! as a holder. A request that starts while the restart waits runs without the
//! cache, as under PHP-FPM, and holds the next restart once this one has run:
//! with `opcache.file_cache` it then loads scripts into the new shared memory.
//!
//! A thread whose one request lasts as long as the thread (a worker, an
//! `oxphp_async()` pool thread) would hold a restart for ever, or run without
//! the cache for ever; it retires at its next idle moment instead and is
//! replaced (see [`retire_reason`]).
//!
//! The module stays inert when OPcache has no lock file this process can find
//! (OPcache off, file cache only, not Linux): every request then starts as it
//! would without it.

#![cfg_attr(not(all(feature = "php", target_os = "linux")), allow(dead_code))]

use crate::plugin::PhpThreadWaker;
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering::SeqCst};
use std::sync::{Mutex, OnceLock, PoisonError, RwLock};
use std::time::Instant;

/// Restart bookkeeping shared by every PHP thread of the process.
///
/// `scheduled` and `done` count restarts: `scheduled` those whose scheduling
/// request has closed, `done` the value of `scheduled` the last restart ran
/// for. A restart is pending while they differ. `holders` counts what runs
/// code from the shared memory, `uncached` the requests opened without the
/// cache that no restart has run for since: the next restart to run makes
/// them holders.
pub(crate) struct Coordinator {
    /// Hook calls: restarts OPcache has scheduled, published or not.
    asked: AtomicU64,
    /// When the last restart was asked for, in [`now_ms`].
    asked_at_ms: AtomicU64,
    /// The value of `asked` the last warning was given for.
    warned_for: AtomicU64,
    scheduled: AtomicU64,
    done: AtomicU64,
    holders: AtomicU64,
    uncached: AtomicU64,
    /// Request startups take it shared; the startup that runs a restart takes
    /// it exclusively, so no other startup runs during the restart.
    window: RwLock<()>,
}

/// What a request was opened with, kept for as long as it is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Opened {
    /// It is a holder from its open, as one opened with the cache, or from
    /// adopting code from the cache (see [`Coordinator::adopt`]). One opened
    /// without the cache is also counted once the restart it opened during
    /// has run (`Coordinator::converted`).
    holds: bool,
    /// It runs with the cache. OPcache turns the cache off for the whole of a
    /// request whose startup found a restart pending.
    cached: bool,
    /// `scheduled` when it was opened, `done` when it adopted.
    seen: u64,
}

/// The lock that keeps OPcache from running a restart on its own.
pub(crate) trait RestartLock {
    /// Let the next request startup run the pending restart.
    fn release(&self);
    /// Keep restarts from running again.
    fn reacquire(&self);
}

/// Why a thread whose request lasts as long as the thread should be replaced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Retire {
    /// Its request holds a restart that only its close lets run — it runs
    /// code from the shared memory, or it scheduled the restart itself.
    Holding,
    /// Its request runs without the cache, and the restart it opened during
    /// has run or nothing holds that restart any more. Its replacement opens
    /// with the cache or runs that restart, unless a later one is pending,
    /// which this request then holds.
    Uncached,
}

impl std::fmt::Display for Retire {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Retire::Holding => "an OPcache restart is waiting for it",
            Retire::Uncached => "it opened while an OPcache restart was pending",
        })
    }
}

/// A restart that has waited longer than `opcache.force_restart_timeout`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Overdue {
    pub waited_ms: u64,
    pub holders: u64,
}

impl Coordinator {
    pub(crate) const fn new() -> Self {
        Self {
            asked: AtomicU64::new(0),
            asked_at_ms: AtomicU64::new(0),
            warned_for: AtomicU64::new(0),
            scheduled: AtomicU64::new(0),
            done: AtomicU64::new(0),
            holders: AtomicU64::new(0),
            uncached: AtomicU64::new(0),
            window: RwLock::new(()),
        }
    }

    /// Run `startup`, the startup of a request, and say what the request was
    /// opened with and whether its startup ran a pending restart; `None` when
    /// the startup failed.
    ///
    /// The restart runs in the startup that finds it pending and nothing
    /// holding it, with `lock` let go for that startup only.
    pub(crate) fn open(
        &self,
        lock: &dyn RestartLock,
        startup: impl FnOnce() -> bool,
    ) -> Option<(Opened, bool)> {
        {
            let _starting = self.window.read().unwrap_or_else(PoisonError::into_inner);
            let seen = self.scheduled.load(SeqCst);
            if seen == self.done.load(SeqCst) {
                return self.open_cached(seen, startup).map(|o| (o, false));
            }
            if self.holders.load(SeqCst) != 0 {
                return self.open_uncached(seen, startup).map(|o| (o, false));
            }
        }
        let _restarting = self.window.write().unwrap_or_else(PoisonError::into_inner);
        // Another startup can have run the restart, or opened with the cache
        // after it, between the two locks.
        let seen = self.scheduled.load(SeqCst);
        if seen == self.done.load(SeqCst) {
            return self.open_cached(seen, startup).map(|o| (o, false));
        }
        if self.holders.load(SeqCst) != 0 {
            return self.open_uncached(seen, startup).map(|o| (o, false));
        }
        self.holders.fetch_add(1, SeqCst);
        lock.release();
        let started = startup();
        lock.reacquire();
        if !started {
            // `done` stays: the next startup runs the restart, or finds it
            // already run and only records it.
            self.release();
            return None;
        }
        // The requests opened without the cache while it waited run on, in
        // memory the restart has just reset.
        self.holders
            .fetch_add(self.uncached.swap(0, SeqCst), SeqCst);
        self.done.store(seen, SeqCst);
        Some((
            Opened {
                holds: true,
                cached: true,
                seen,
            },
            true,
        ))
    }

    fn open_cached(&self, seen: u64, startup: impl FnOnce() -> bool) -> Option<Opened> {
        self.holders.fetch_add(1, SeqCst);
        if startup() {
            Some(Opened {
                holds: true,
                cached: true,
                seen,
            })
        } else {
            self.release();
            None
        }
    }

    /// Called inside the startup window, which keeps the count apart from a
    /// restart's taking it.
    fn open_uncached(&self, seen: u64, startup: impl FnOnce() -> bool) -> Option<Opened> {
        self.uncached.fetch_add(1, SeqCst);
        if startup() {
            Some(Opened {
                holds: false,
                cached: false,
                seen,
            })
        } else {
            self.drop_uncached();
            None
        }
    }

    fn drop_uncached(&self) {
        let before = self.uncached.fetch_sub(1, SeqCst);
        debug_assert!(before > 0, "OPcache uncached request counted out twice");
    }

    /// Whether a restart has run since `opened`, a request opened without the
    /// cache, opened: that restart made it a holder.
    fn converted(&self, opened: Opened) -> bool {
        self.done.load(SeqCst) >= opened.seen
    }

    /// The request opened with `opened` has shut down. True when it was the
    /// last holder of a pending restart.
    pub(crate) fn close(&self, opened: Opened) -> bool {
        if opened.holds {
            return self.release();
        }
        let _closing = self.window.read().unwrap_or_else(PoisonError::into_inner);
        if self.converted(opened) {
            self.release()
        } else {
            self.drop_uncached();
            false
        }
    }

    /// The request open with `opened` runs code from the shared memory from
    /// here on: what it is open with now. It holds restarts until it closes.
    pub(crate) fn adopt(&self, opened: Opened) -> Opened {
        if opened.holds {
            return opened;
        }
        let _adopting = self.window.read().unwrap_or_else(PoisonError::into_inner);
        if !self.converted(opened) {
            self.drop_uncached();
            self.holders.fetch_add(1, SeqCst);
        }
        // No restart runs while it holds, so `done` stays the memory it uses.
        Opened {
            holds: true,
            cached: false,
            seen: self.done.load(SeqCst),
        }
    }

    /// Restarts whose scheduling request has closed. From here on, a request
    /// that starts runs without the cache until one of them has run.
    pub(crate) fn publish(&self, restarts: u64) {
        self.scheduled.fetch_add(restarts, SeqCst);
    }

    /// OPcache has scheduled a restart. Only atomics: it runs inside OPcache,
    /// under its shared memory lock.
    pub(crate) fn ask(&self, now_ms: u64) {
        self.asked_at_ms.store(now_ms, SeqCst);
        self.asked.fetch_add(1, SeqCst);
    }

    /// A holder for code from the shared memory that outlives the request
    /// that has it. The count rises from zero here only for a closure that
    /// `opcache.preload` loaded, dispatched by a request without the cache:
    /// other code in the shared memory is reached only from what holds
    /// already. A restart keeps preloaded opcodes, so that rise races a
    /// restart no more than such a request calling preloaded code does.
    pub(crate) fn hold(&'static self) -> Hold {
        self.holders.fetch_add(1, SeqCst);
        Hold(self)
    }

    /// True when this was the last holder of a pending restart.
    fn release(&self) -> bool {
        let before = self.holders.fetch_sub(1, SeqCst);
        debug_assert!(before > 0, "OPcache restart holder released twice");
        before == 1 && self.scheduled.load(SeqCst) != self.done.load(SeqCst)
    }

    /// Whether a thread whose open request is `opened` should be replaced at
    /// its idle moment; `scheduled_here` when the thread has scheduled a
    /// restart its request has not published yet.
    pub(crate) fn retire(&self, opened: Opened, scheduled_here: bool) -> Option<Retire> {
        if scheduled_here {
            return Some(Retire::Holding);
        }
        if opened.holds {
            if self.scheduled.load(SeqCst) != opened.seen {
                return Some(Retire::Holding);
            }
            // One that adopted still runs without the cache, and with no
            // restart pending, the one it opened during has run.
            return (!opened.cached).then_some(Retire::Uncached);
        }
        // A request without the cache was opened while a restart was pending.
        // Once that restart has run, the request holds the next one, and its
        // replacement starts with the cache unless the next is pending too.
        // While that restart waits, the replacement runs it once nothing holds
        // it: waiting only for it to run would leave a pool that only has such
        // threads with no one to run it.
        (self.converted(opened) || self.holders.load(SeqCst) == 0).then_some(Retire::Uncached)
    }

    /// A restart OPcache asked for and that has not run after `timeout_ms`;
    /// once per restart. `timeout_ms == 0` never reports.
    pub(crate) fn overdue(&self, now_ms: u64, timeout_ms: u64) -> Option<Overdue> {
        let asked = self.asked.load(SeqCst);
        if timeout_ms == 0 || asked == self.done.load(SeqCst) {
            return None;
        }
        let waited_ms = now_ms.saturating_sub(self.asked_at_ms.load(SeqCst));
        if waited_ms < timeout_ms || self.warned_for.swap(asked, SeqCst) == asked {
            return None;
        }
        Some(Overdue {
            waited_ms,
            holders: self.holders.load(SeqCst),
        })
    }

    /// Milliseconds since the last restart was asked for.
    fn waited_ms(&self, now_ms: u64) -> u64 {
        now_ms.saturating_sub(self.asked_at_ms.load(SeqCst))
    }
}

/// A holder taken for an `oxphp_async()` task whose code is in the shared
/// memory: the restart waits until the task has ended and this is dropped.
pub struct Hold(&'static Coordinator);

impl Drop for Hold {
    fn drop(&mut self) {
        if self.0.release() {
            wake_all();
        }
    }
}

impl std::fmt::Debug for Hold {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Hold")
    }
}

/// Whether `target`, what `/proc/self/fd/N` links to, is OPcache's lock file:
/// the memfd it creates on Linux, or the unlinked file it falls back to.
pub(crate) fn is_lock_file(target: &str) -> bool {
    target.starts_with("/memfd:opcache_lock ") || target.contains("/.ZendSem.")
}

static COORDINATOR: Coordinator = Coordinator::new();

/// The lock taken at startup; unset while the module is inert.
static LOCK: OnceLock<Box<dyn RestartLock + Send + Sync>> = OnceLock::new();

/// `opcache.force_restart_timeout`, in milliseconds; 0 disables the warning.
static FORCE_RESTART_TIMEOUT_MS: AtomicU64 = AtomicU64::new(0);

/// The reason OPcache gave for the last restart it scheduled.
static REASON: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(-1);

static EPOCH: OnceLock<Instant> = OnceLock::new();

fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

thread_local! {
    /// The request open on this thread, when the module opened it.
    static OPENED: Cell<Option<Opened>> = const { Cell::new(None) };
    /// Restarts OPcache scheduled on this thread that no closed request has
    /// published yet.
    static SCHEDULED_HERE: Cell<u64> = const { Cell::new(0) };
}

/// Wakers of the worker threads waiting for work, so that one whose request
/// should retire notices at once instead of at its next request.
static WAKERS: Mutex<Vec<(u64, PhpThreadWaker)>> = Mutex::new(Vec::new());
static NEXT_WAKER: AtomicU64 = AtomicU64::new(0);

/// Removes its waker from the registry when dropped.
pub(crate) struct WakerRegistration(u64);

/// Have the calling thread's `waker` woken whenever a thread may have to
/// retire, for as long as the returned registration lives.
pub(crate) fn register_waker(waker: PhpThreadWaker) -> WakerRegistration {
    let id = NEXT_WAKER.fetch_add(1, SeqCst);
    WAKERS
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .push((id, waker));
    WakerRegistration(id)
}

impl Drop for WakerRegistration {
    fn drop(&mut self) {
        WAKERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|(id, _)| *id != self.0);
    }
}

fn wake_all() {
    for (_, waker) in WAKERS.lock().unwrap_or_else(PoisonError::into_inner).iter() {
        waker.wake();
    }
}

fn reason_text(reason: i32) -> &'static str {
    // zend_accel_restart_reason, ext/opcache/ZendAccelerator.h (8.4, 8.5).
    match reason {
        0 => "out of memory",
        1 => "hash overflow",
        2 => "opcache_reset()",
        _ => "unknown",
    }
}

/// Publish the restarts OPcache scheduled on this thread. Called once the
/// request they were scheduled in is gone: OPcache marks a restart pending
/// only after the hook has returned, so by the close it is marked, and a
/// startup that sees it published also sees it pending.
fn publish_scheduled_here() {
    let restarts = SCHEDULED_HERE.with(|n| n.replace(0));
    if restarts == 0 {
        return;
    }
    COORDINATOR.publish(restarts);
    tracing::info!(
        reason = reason_text(REASON.load(SeqCst)),
        "OPcache restart scheduled: requests that start from now on run without \
         the cache, and the restart runs once nothing runs code from the shared \
         memory it resets"
    );
    wake_all();
}

/// Start a request with `startup`, the call to `php_request_startup`, which
/// returns whether it succeeded; the restart protocol of this module wraps it
/// unless the module is inert.
pub(crate) fn start_request(startup: impl FnOnce() -> bool) -> bool {
    let Some(lock) = LOCK.get() else {
        return startup();
    };
    match COORDINATOR.open(lock.as_ref(), startup) {
        Some((opened, restarted)) => {
            OPENED.with(|o| o.set(Some(opened)));
            if restarted {
                tracing::info!(
                    waited_ms = COORDINATOR.waited_ms(now_ms()),
                    "OPcache restart done"
                );
                wake_all();
            }
            true
        }
        None => {
            // No close follows a failed startup: what OPcache scheduled in it
            // is published here, and a thread waiting for the holder this
            // startup gave back is told.
            publish_scheduled_here();
            wake_all();
            false
        }
    }
}

/// The request open on this thread has shut down. Called after
/// `php_request_shutdown`, which is the last of it that touches the shared
/// memory.
pub(crate) fn request_closed() {
    publish_scheduled_here();
    if let Some(opened) = OPENED.with(Cell::take) {
        if COORDINATOR.close(opened) {
            wake_all();
        }
    }
}

/// A holder for an `oxphp_async()` task whose code is in the shared memory,
/// taken by the request that dispatches it; `None` while the module is inert.
pub(crate) fn task_hold() -> Option<Hold> {
    LOCK.get()?;
    Some(COORDINATOR.hold())
}

/// The request open on this thread is about to run code from the shared
/// memory that it did not compile or load itself: a task's, or a callable
/// another request made. What that code leaves in the request (a function it
/// declares, its run-time cache) outlives the call, so the request holds
/// restarts from here until it closes. Nothing while no request is open on
/// the thread, as during a startup.
pub(crate) fn adopt() {
    if LOCK.get().is_none() {
        return;
    }
    OPENED.with(|o| {
        if let Some(opened) = o.get() {
            o.set(Some(COORDINATOR.adopt(opened)));
        }
    });
}

/// Why the calling thread, whose request lasts as long as the thread, should
/// be replaced now; called only at a moment it runs nothing.
pub(crate) fn retire_reason() -> Option<Retire> {
    let opened = OPENED.with(Cell::get)?;
    COORDINATOR.retire(opened, SCHEDULED_HERE.with(Cell::get) > 0)
}

/// Log a warning, once per restart, when a restart has waited longer than
/// `opcache.force_restart_timeout`. Called on the supervisor's tick.
pub(crate) fn report_waiting() {
    let timeout_ms = FORCE_RESTART_TIMEOUT_MS.load(SeqCst);
    if let Some(o) = COORDINATOR.overdue(now_ms(), timeout_ms) {
        tracing::warn!(
            waited_secs = o.waited_ms / 1000,
            holders = o.holders,
            "OPcache restart still waiting, longer than opcache.force_restart_timeout: \
             requests or oxphp_async() tasks still run code from the shared memory it \
             resets. OxPHP stops none of them; the restart runs once they have ended, \
             and until then requests run without the cache"
        );
    }
}

#[cfg(all(feature = "php", target_os = "linux"))]
mod ofd {
    use super::RestartLock;
    use std::fs::File;
    use std::os::fd::AsRawFd;

    /// A lock on byte 1 of OPcache's lock file, owned by an open file
    /// description of its own: OPcache's `F_GETLK` reports it to this process,
    /// and the whole-file unlock OPcache does at the end of each request
    /// (`accel_unlock_all`) releases only the process's own locks.
    pub(super) struct OfdLock(File);

    impl OfdLock {
        /// Find OPcache's lock file among this process's descriptors and take
        /// the lock. `Ok(None)` when there is no such file.
        pub(super) fn take() -> Result<Option<Self>, String> {
            let mut found = Vec::new();
            for entry in std::fs::read_dir("/proc/self/fd").map_err(|e| e.to_string())? {
                let Ok(entry) = entry else { continue };
                let Ok(target) = std::fs::read_link(entry.path()) else {
                    continue;
                };
                if target.to_str().is_some_and(super::is_lock_file) {
                    found.push(entry.path());
                }
            }
            let path = match found.as_slice() {
                [] => return Ok(None),
                [one] => one,
                _ => return Err(format!("{} candidate lock files", found.len())),
            };
            // A new open of the file through procfs, not a dup: a dup shares
            // the description, and the lock with it, with OPcache's own
            // descriptor.
            let file = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let lock = OfdLock(file);
            lock.set(libc::F_OFD_SETLK, libc::F_RDLCK)
                .map_err(|e| format!("F_OFD_SETLK: {e}"))?;
            Ok(Some(lock))
        }

        fn set(&self, cmd: libc::c_int, kind: libc::c_int) -> std::io::Result<()> {
            // SAFETY: an all-zero `flock` is a valid value; the fields the
            // call reads are set below.
            let mut fl: libc::flock = unsafe { std::mem::zeroed() };
            fl.l_type = kind as libc::c_short;
            fl.l_whence = libc::SEEK_SET as libc::c_short;
            // The byte OPcache's requests read-lock and its restart test
            // (`accel_is_inactive`) asks about. `l_pid` stays 0, as the OFD
            // commands require.
            fl.l_start = 1;
            fl.l_len = 1;
            loop {
                // SAFETY: the descriptor is open for as long as `self`, and
                // `fl` is a valid `flock` for the duration of the call.
                if unsafe { libc::fcntl(self.0.as_raw_fd(), cmd, &mut fl) } == 0 {
                    return Ok(());
                }
                let err = std::io::Error::last_os_error();
                if err.kind() != std::io::ErrorKind::Interrupted {
                    return Err(err);
                }
            }
        }
    }

    impl RestartLock for OfdLock {
        fn release(&self) {
            if let Err(e) = self.set(libc::F_OFD_SETLK, libc::F_UNLCK) {
                tracing::error!(error = %e, "could not let go of the OPcache restart lock; the pending restart will not run");
            }
        }

        fn reacquire(&self) {
            // Nothing takes a write lock on this byte, so the wait is for show.
            if let Err(e) = self.set(libc::F_OFD_SETLKW, libc::F_RDLCK) {
                tracing::error!(error = %e, "could not take the OPcache restart lock back; OPcache restarts are no longer held while requests run cached code");
            }
        }
    }
}

/// OPcache's hook for a restart it schedules, called before it marks the
/// restart pending, under its shared memory lock, on whichever thread ran
/// out of memory or called `opcache_reset()`: atomics and this thread's
/// counter only.
#[cfg(all(feature = "php", target_os = "linux"))]
unsafe extern "C" fn on_restart_scheduled(reason: std::os::raw::c_int) {
    REASON.store(reason, SeqCst);
    COORDINATOR.ask(now_ms());
    SCHEDULED_HERE.with(|n| n.set(n.get() + 1));
    if let Some(Some(previous)) = PREVIOUS_HOOK.get() {
        // SAFETY: the hook another extension installed before this one, called
        // with the reason OPcache passed, as OPcache would have called it.
        unsafe { previous(reason) };
    }
}

#[cfg(all(feature = "php", target_os = "linux"))]
type RestartHook = Option<unsafe extern "C" fn(std::os::raw::c_int)>;

#[cfg(all(feature = "php", target_os = "linux"))]
static PREVIOUS_HOOK: OnceLock<RestartHook> = OnceLock::new();

/// Take the restart lock and hook OPcache's restart scheduling. Called once,
/// after `php_module_startup` (which creates the lock file) and before any
/// request starts.
#[cfg(feature = "php")]
pub(crate) fn install() {
    EPOCH.get_or_init(Instant::now);
    #[cfg(target_os = "linux")]
    {
        let lock = match ofd::OfdLock::take() {
            Ok(Some(lock)) => lock,
            Ok(None) => {
                tracing::debug!("OPcache restarts not coordinated: no OPcache lock file");
                return;
            }
            Err(e) => {
                tracing::warn!(error = %e, "OPcache restarts not coordinated: a restart may reset the shared memory under running requests");
                return;
            }
        };
        // SAFETY: a NUL-terminated name; RTLD_DEFAULT searches the global
        // scope, which libphp.so is loaded into.
        let symbol = unsafe {
            libc::dlsym(
                libc::RTLD_DEFAULT,
                c"zend_accel_schedule_restart_hook".as_ptr(),
            )
        };
        if symbol.is_null() {
            tracing::debug!(
                "OPcache restarts not coordinated: no zend_accel_schedule_restart_hook"
            );
            return;
        }
        let name = c"opcache.force_restart_timeout";
        // SAFETY: after module startup; `name` is NUL-terminated and its
        // length excludes the terminator, as `zend_ini_long` expects.
        let timeout_secs =
            unsafe { crate::php::bindings::zend_ini_long(name.as_ptr(), name.to_bytes().len(), 0) };
        FORCE_RESTART_TIMEOUT_MS.store(timeout_secs.max(0) as u64 * 1000, SeqCst);
        let slot = symbol.cast::<RestartHook>();
        // SAFETY: `slot` is the engine's `zend_accel_schedule_restart_hook`, a
        // `void (*)(int)` the engine reads only while it schedules a restart,
        // which no request can do yet: no request has started.
        let previous = unsafe { slot.read() };
        let _ = PREVIOUS_HOOK.set(previous);
        if LOCK.set(Box::new(lock)).is_err() {
            return;
        }
        // SAFETY: as for the read above.
        unsafe { slot.write(Some(on_restart_scheduled)) };
    }
    #[cfg(not(target_os = "linux"))]
    tracing::debug!("OPcache restarts not coordinated on this platform");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    use std::time::Duration;

    #[derive(Default)]
    struct FakeLock {
        released: AtomicBool,
        releases: AtomicU64,
    }

    impl RestartLock for FakeLock {
        fn release(&self) {
            assert!(!self.released.swap(true, SeqCst), "released twice");
            self.releases.fetch_add(1, SeqCst);
        }
        fn reacquire(&self) {
            assert!(self.released.swap(false, SeqCst), "reacquired unreleased");
        }
    }

    fn coordinator() -> &'static Coordinator {
        Box::leak(Box::new(Coordinator::new()))
    }

    /// Open a request whose startup succeeds and records whether the lock was
    /// let go during it.
    fn open(c: &Coordinator, lock: &FakeLock) -> (Opened, bool, bool) {
        let mut released_during = false;
        let (opened, restarted) = c
            .open(lock, || {
                released_during = lock.released.load(SeqCst);
                true
            })
            .expect("startup succeeds");
        assert!(
            !lock.released.load(SeqCst),
            "lock held again after the startup"
        );
        (opened, restarted, released_during)
    }

    #[test]
    fn no_restart_pending_opens_cached_and_keeps_the_lock() {
        let c = coordinator();
        let lock = FakeLock::default();
        let (opened, restarted, released) = open(c, &lock);
        assert!(opened.holds);
        assert!(!restarted && !released);
        assert!(!c.close(opened));
        assert_eq!(c.holders.load(SeqCst), 0);
    }

    #[test]
    fn restart_waits_for_a_cached_request_and_runs_after_it() {
        let c = coordinator();
        let lock = FakeLock::default();
        let (a, ..) = open(c, &lock);
        c.ask(0);
        c.publish(1);

        let (b, restarted, released) = open(c, &lock);
        assert!(!b.holds, "opened while the restart waits: no cache");
        assert!(!restarted && !released, "a holder is still open");

        assert!(c.close(a), "the last holder frees the restart");
        assert!(!c.close(b));

        let (r, restarted, released) = open(c, &lock);
        assert!(restarted && released, "the next startup runs the restart");
        assert!(r.holds);
        assert_eq!(lock.releases.load(SeqCst), 1);
        assert_eq!(c.done.load(SeqCst), 1);

        let (d, restarted, released) = open(c, &lock);
        assert!(d.holds && !restarted && !released);
    }

    #[test]
    fn task_hold_keeps_the_restart_after_its_request_closes() {
        let c = coordinator();
        let lock = FakeLock::default();
        let (a, ..) = open(c, &lock);
        let task = c.hold();
        c.ask(0);
        c.publish(1);
        assert!(!c.close(a), "the task still holds the restart");

        let (b, restarted, released) = open(c, &lock);
        assert!(!b.holds && !restarted && !released);
        assert_eq!(c.holders.load(SeqCst), 1);

        drop(task);
        assert_eq!(c.holders.load(SeqCst), 0);
        let (_, restarted, _) = open(c, &lock);
        assert!(restarted);
    }

    #[test]
    fn failed_restarting_startup_leaves_the_restart_pending() {
        let c = coordinator();
        let lock = FakeLock::default();
        c.ask(0);
        c.publish(1);
        assert!(c.open(&lock, || false).is_none());
        assert!(!lock.released.load(SeqCst));
        assert_eq!(c.done.load(SeqCst), 0);
        assert_eq!(c.holders.load(SeqCst), 0);

        let (_, restarted, released) = open(c, &lock);
        assert!(restarted && released, "the next startup tries again");
    }

    #[test]
    fn failed_cached_startup_gives_its_holder_back() {
        let c = coordinator();
        let lock = FakeLock::default();
        assert!(c.open(&lock, || false).is_none());
        assert_eq!(c.holders.load(SeqCst), 0);
    }

    #[test]
    fn retire_first_wave_cached_request_after_a_restart_is_published() {
        let c = coordinator();
        let lock = FakeLock::default();
        let (w, ..) = open(c, &lock);
        assert_eq!(c.retire(w, false), None);
        assert_eq!(c.retire(w, true), Some(Retire::Holding), "it scheduled one");
        c.ask(0);
        assert_eq!(c.retire(w, false), None, "not published yet");
        c.publish(1);
        assert_eq!(c.retire(w, false), Some(Retire::Holding));
    }

    #[test]
    fn retire_second_wave_uncached_request_once_the_restart_ran() {
        let c = coordinator();
        let lock = FakeLock::default();
        let (holder, ..) = open(c, &lock);
        c.ask(0);
        c.publish(1);
        let (u, ..) = open(c, &lock);
        let (u2, ..) = open(c, &lock);
        assert_eq!(c.retire(u, false), None, "the restart is still held");

        c.close(holder);
        assert_eq!(
            c.retire(u, false),
            Some(Retire::Uncached),
            "nothing holds the restart: a replacement can run it"
        );
        let (_, restarted, _) = open(c, &lock);
        assert!(restarted);
        assert_eq!(
            c.retire(u2, false),
            Some(Retire::Uncached),
            "the restart ran"
        );
    }

    #[test]
    fn a_restarter_or_a_request_after_it_does_not_retire() {
        let c = coordinator();
        let lock = FakeLock::default();
        c.ask(0);
        c.publish(1);
        let (r, restarted, _) = open(c, &lock);
        assert!(restarted);
        let (d, ..) = open(c, &lock);
        assert_eq!(c.retire(r, false), None);
        assert_eq!(c.retire(d, false), None);
    }

    #[test]
    fn overdue_reports_once_per_restart_and_not_when_disabled() {
        let c = coordinator();
        let lock = FakeLock::default();
        c.ask(1_000);
        assert_eq!(c.overdue(2_000, 2_000), None, "not long enough");
        assert_eq!(c.overdue(5_000, 0), None, "disabled");
        assert_eq!(
            c.overdue(3_500, 2_000),
            Some(Overdue {
                waited_ms: 2_500,
                holders: 0
            })
        );
        assert_eq!(c.overdue(9_000, 2_000), None, "once");

        c.publish(1);
        let (_, restarted, _) = open(c, &lock);
        assert!(restarted);
        assert_eq!(c.overdue(20_000, 2_000), None, "it ran");

        c.ask(30_000);
        assert!(
            c.overdue(33_000, 2_000).is_some(),
            "the next restart warns again"
        );
    }

    /// Hold the startup window shared, as a startup in progress does, while
    /// `open` on another thread gets past its shared check and waits for the
    /// exclusive lock; run `meanwhile`, then let it go on. Returns what it
    /// opened and how often it let go of the lock.
    fn open_between_the_locks(c: &'static Coordinator, meanwhile: impl FnOnce()) -> (Opened, u64) {
        let lock: &'static FakeLock = Box::leak(Box::default());
        let starting = c.window.read().unwrap();
        let opener = std::thread::spawn(move || c.open(lock, || true).unwrap().0);
        // A reader is refused once a writer waits: the opener is past its
        // shared check, at the exclusive lock.
        let deadline = Instant::now() + Duration::from_secs(5);
        while let Ok(again) = c.window.try_read() {
            drop(again);
            assert!(
                Instant::now() < deadline,
                "the opener never waited for the exclusive lock"
            );
            std::thread::yield_now();
        }
        meanwhile();
        drop(starting);
        (opener.join().unwrap(), lock.releases.load(SeqCst))
    }

    #[test]
    fn a_restart_run_between_the_two_locks_opens_the_request_cached() {
        let c = coordinator();
        c.ask(0);
        c.publish(1);
        // Another startup runs the restart, and keeps its request open.
        let (opened, releases) = open_between_the_locks(c, || {
            c.done.store(1, SeqCst);
            c.holders.fetch_add(1, SeqCst);
        });
        assert!(
            opened.holds,
            "its startup finds the restart run: the cache is on"
        );
        assert_eq!(releases, 0);
    }

    #[test]
    fn a_holder_taken_between_the_two_locks_keeps_the_restart_back() {
        let c = coordinator();
        c.ask(0);
        c.publish(1);
        // A startup that read the counts before the restart was published
        // takes its holder.
        let (opened, releases) = open_between_the_locks(c, || {
            c.holders.fetch_add(1, SeqCst);
        });
        assert!(!opened.holds);
        assert_eq!(releases, 0, "no restart under a holder");
    }

    #[test]
    fn concurrent_startups_run_one_restart_and_none_under_a_holder() {
        for _ in 0..50 {
            let c = coordinator();
            let lock = Arc::new(FakeLock::default());
            let in_startup = Arc::new(AtomicU64::new(0));
            c.ask(0);
            c.publish(1);
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    let lock = Arc::clone(&lock);
                    let in_startup = Arc::clone(&in_startup);
                    std::thread::spawn(move || {
                        let (o, restarted) = c
                            .open(lock.as_ref(), || {
                                in_startup.fetch_add(1, SeqCst);
                                if lock.released.load(SeqCst) {
                                    // A startup that overlapped this one could
                                    // see the restart run and open cached
                                    // without holding.
                                    std::thread::sleep(Duration::from_millis(2));
                                    assert_eq!(in_startup.load(SeqCst), 1);
                                    assert_eq!(c.holders.load(SeqCst), 1);
                                }
                                in_startup.fetch_sub(1, SeqCst);
                                true
                            })
                            .unwrap();
                        c.close(o);
                        restarted
                    })
                })
                .collect();
            let restarts = threads
                .into_iter()
                .map(|t| t.join().unwrap())
                .filter(|r| *r)
                .count();
            assert_eq!(restarts, 1);
            assert_eq!(lock.releases.load(SeqCst), 1);
            assert_eq!(c.holders.load(SeqCst), 0);
            assert_eq!(c.uncached.load(SeqCst), 0);
        }
    }

    #[test]
    fn requests_opened_while_a_restart_waits_hold_the_next_one() {
        let c = coordinator();
        let lock = FakeLock::default();
        let (a, ..) = open(c, &lock);
        c.ask(0);
        c.publish(1);
        let (u, ..) = open(c, &lock);
        assert!(!u.holds);
        assert_eq!(c.holders.load(SeqCst), 1, "it does not hold this restart");
        assert!(c.close(a));

        let (r, restarted, _) = open(c, &lock);
        assert!(restarted);
        assert_eq!(c.holders.load(SeqCst), 2, "the restart made it a holder");
        assert_eq!(c.uncached.load(SeqCst), 0);

        c.ask(0);
        c.publish(1);
        assert!(!c.close(r), "it still holds the second restart");
        let (v, restarted, released) = open(c, &lock);
        assert!(!v.holds && !restarted && !released);
        assert!(c.close(u), "the last holder of the second restart");
        assert!(!c.close(v));
        let (_, restarted, _) = open(c, &lock);
        assert!(restarted);
    }

    #[test]
    fn a_request_without_the_cache_closed_before_the_restart_holds_nothing() {
        let c = coordinator();
        let lock = FakeLock::default();
        let (a, ..) = open(c, &lock);
        c.ask(0);
        c.publish(1);
        let (u, ..) = open(c, &lock);
        assert!(c.open(&lock, || false).is_none());
        assert_eq!(
            c.uncached.load(SeqCst),
            1,
            "a failed startup is not counted"
        );
        assert!(!c.close(u));
        assert_eq!(c.uncached.load(SeqCst), 0);
        assert!(c.close(a));
        let (_, restarted, _) = open(c, &lock);
        assert!(restarted);
        assert_eq!(c.holders.load(SeqCst), 1, "only the restarter");
    }

    #[test]
    fn an_adopting_request_holds_the_restart_and_retires() {
        let c = coordinator();
        let lock = FakeLock::default();
        let (a, ..) = open(c, &lock);
        c.ask(0);
        c.publish(1);
        let (t, ..) = open(c, &lock);
        let task = c.hold();
        let t = c.adopt(t);
        assert!(t.holds);
        assert_eq!(c.uncached.load(SeqCst), 0);
        assert_eq!(c.retire(t, false), Some(Retire::Holding));

        assert!(!c.close(a));
        drop(task);
        assert_eq!(
            c.holders.load(SeqCst),
            1,
            "the task is gone, its traces are not"
        );
        let (b, restarted, _) = open(c, &lock);
        assert!(!b.holds && !restarted);
        assert!(c.close(t));
        assert!(!c.close(b));
        let (_, restarted, _) = open(c, &lock);
        assert!(restarted);
    }

    #[test]
    fn adopting_after_the_restart_counts_the_request_once() {
        let c = coordinator();
        let lock = FakeLock::default();
        let (a, ..) = open(c, &lock);
        c.ask(0);
        c.publish(1);
        let (t, ..) = open(c, &lock);
        c.close(a);
        let (r, restarted, _) = open(c, &lock);
        assert!(restarted);
        assert_eq!(c.holders.load(SeqCst), 2);

        let t = c.adopt(t);
        assert_eq!(c.holders.load(SeqCst), 2, "the restart counted it already");
        assert_eq!(
            c.retire(t, false),
            Some(Retire::Uncached),
            "it runs without the cache"
        );
        assert_eq!(c.adopt(t), t, "a holder adopts nothing");
        assert!(!c.close(t));
        assert!(!c.close(r));
        assert_eq!(c.holders.load(SeqCst), 0);
    }

    #[test]
    fn closes_racing_a_restart_leave_no_count_behind() {
        for _ in 0..50 {
            let c = coordinator();
            let lock = Arc::new(FakeLock::default());
            let (a, ..) = open(c, &lock);
            c.ask(0);
            c.publish(1);
            let threads: Vec<_> = (0..8)
                .map(|_| {
                    let lock = Arc::clone(&lock);
                    std::thread::spawn(move || {
                        for _ in 0..20 {
                            let (o, _) = c.open(lock.as_ref(), || true).unwrap();
                            c.close(o);
                        }
                    })
                })
                .collect();
            c.close(a);
            for t in threads {
                t.join().unwrap();
            }
            let (o, ..) = open(c, &lock);
            c.close(o);
            assert_eq!(c.done.load(SeqCst), 1);
            assert_eq!(c.holders.load(SeqCst), 0);
            assert_eq!(c.uncached.load(SeqCst), 0);
        }
    }

    /// Open a request without the cache while a restart waits, then let
    /// nothing hold the restart.
    fn uncached_request_before_a_restart(c: &'static Coordinator) -> Opened {
        let lock = FakeLock::default();
        let (a, ..) = open(c, &lock);
        c.ask(0);
        c.publish(1);
        let (u, ..) = open(c, &lock);
        assert!(c.close(a));
        u
    }

    /// Run `meanwhile` on one thread while the startup of another runs the
    /// pending restart; say whether `meanwhile` returned within 50 ms of its
    /// thread reaching it, before that startup ended.
    fn while_a_restart_runs<T: Send + 'static>(
        c: &'static Coordinator,
        meanwhile: impl FnOnce() -> T + Send + 'static,
    ) -> (bool, T) {
        let lock: &'static FakeLock = Box::leak(Box::default());
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let restarter = std::thread::spawn(move || {
            c.open(lock, || {
                started_tx.send(()).unwrap();
                go_rx.recv().unwrap();
                true
            })
            .unwrap()
        });
        started_rx.recv().unwrap();
        let returned = Arc::new(AtomicBool::new(false));
        let (calling_tx, calling_rx) = std::sync::mpsc::channel();
        let other = std::thread::spawn({
            let returned = Arc::clone(&returned);
            move || {
                calling_tx.send(()).unwrap();
                let v = meanwhile();
                returned.store(true, SeqCst);
                v
            }
        });
        // Timed from the call, not from the spawn: a thread slow to start
        // would let a `meanwhile` that does not wait pass.
        calling_rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(50));
        let early = returned.load(SeqCst);
        go_tx.send(()).unwrap();
        assert!(restarter.join().unwrap().1, "the startup ran the restart");
        (early, other.join().unwrap())
    }

    #[test]
    fn a_close_without_the_cache_waits_for_a_restart_in_progress() {
        let c = coordinator();
        let u = uncached_request_before_a_restart(c);
        let (early, last) = while_a_restart_runs(c, move || c.close(u));
        assert!(!early, "it would race the restart taking it as a holder");
        assert!(!last);
        assert_eq!(c.holders.load(SeqCst), 1, "the restarter alone");
        assert_eq!(c.uncached.load(SeqCst), 0);
    }

    #[test]
    fn adopting_waits_for_a_restart_in_progress() {
        let c = coordinator();
        let u = uncached_request_before_a_restart(c);
        let (early, adopted) = while_a_restart_runs(c, move || c.adopt(u));
        assert!(!early, "it would race the restart taking it as a holder");
        assert_eq!(
            adopted,
            Opened {
                holds: true,
                cached: false,
                seen: 1
            }
        );
        assert_eq!(c.holders.load(SeqCst), 2);
        assert_eq!(c.uncached.load(SeqCst), 0);
    }

    #[test]
    fn lock_file_names() {
        assert!(is_lock_file("/memfd:opcache_lock (deleted)"));
        assert!(is_lock_file("/tmp/.ZendSem.AbC123 (deleted)"));
        assert!(!is_lock_file("/memfd:opcache_jit (deleted)"));
        assert!(!is_lock_file("/dev/null"));
        assert!(!is_lock_file("/tmp/#12345 (deleted)"));
    }
}
