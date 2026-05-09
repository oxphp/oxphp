//! Per-worker mutable state observed by the supervisor.
//!
//! The supervisor reads these atomics once per second; the worker
//! writes `request_start_us` on each request and bumps `ticks` once
//! per PHP function call (via the Zend fcall observer).

use std::sync::atomic::AtomicU64;

#[repr(C, align(64))]
pub struct WorkerHeartbeat {
    pub request_start_us: AtomicU64,
    pub last_cpu_us: AtomicU64,
    pub ticks: AtomicU64,
    pub last_ticks: AtomicU64,
    pub tid: AtomicU64,
}

impl WorkerHeartbeat {
    pub fn new() -> Self {
        Self {
            request_start_us: AtomicU64::new(0),
            last_cpu_us: AtomicU64::new(0),
            ticks: AtomicU64::new(0),
            last_ticks: AtomicU64::new(0),
            tid: AtomicU64::new(0),
        }
    }
}

impl Default for WorkerHeartbeat {
    fn default() -> Self {
        Self::new()
    }
}

/// Microseconds since process start. Single source of truth for
/// timestamps on heartbeat fields and supervisor scans.
pub fn monotonic_us() -> u64 {
    use std::sync::OnceLock;
    static EPOCH: OnceLock<std::time::Instant> = OnceLock::new();
    let epoch = EPOCH.get_or_init(std::time::Instant::now);
    std::time::Instant::now()
        .saturating_duration_since(*epoch)
        .as_micros() as u64
}

/// Linux `gettid` / Darwin `pthread_threadid_np`. 0 if unsupported.
pub fn current_tid() -> u64 {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::syscall(libc::SYS_gettid) as u64
    }
    #[cfg(target_os = "macos")]
    unsafe {
        let mut tid: u64 = 0;
        // Passing 0 as the pthread_t means "the calling thread".
        if libc::pthread_threadid_np(0, &mut tid) == 0 {
            tid
        } else {
            0
        }
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    #[test]
    fn alignment_is_64() {
        assert_eq!(std::mem::align_of::<WorkerHeartbeat>(), 64);
    }

    #[test]
    fn new_is_zeroed() {
        let h = WorkerHeartbeat::new();
        assert_eq!(h.request_start_us.load(Ordering::Relaxed), 0);
        assert_eq!(h.last_cpu_us.load(Ordering::Relaxed), 0);
        assert_eq!(h.ticks.load(Ordering::Relaxed), 0);
        assert_eq!(h.last_ticks.load(Ordering::Relaxed), 0);
        assert_eq!(h.tid.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn monotonic_us_advances() {
        let a = monotonic_us();
        std::thread::sleep(std::time::Duration::from_micros(100));
        let b = monotonic_us();
        assert!(b >= a);
    }

    #[test]
    fn current_tid_nonzero_on_supported_os() {
        let tid = current_tid();
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        assert_ne!(tid, 0);
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        let _ = tid;
    }
}
