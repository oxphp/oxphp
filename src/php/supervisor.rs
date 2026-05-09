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
        read_thread_cpu_us_linux(tid)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = tid;
        None
    }
}

#[cfg(target_os = "linux")]
fn read_thread_cpu_us_linux(tid: u64) -> Option<u64> {
    use std::io::Read;
    let path = format!("/proc/self/task/{}/stat", tid);
    let mut buf = String::with_capacity(512);
    std::fs::File::open(&path)
        .ok()?
        .read_to_string(&mut buf)
        .ok()?;
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
}
