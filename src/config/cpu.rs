//! How many CPUs the `PHP_WORKERS` and `TOKIO_WORKERS` defaults are sized
//! from, and which limit set that number.
//!
//! The count is `std::thread::available_parallelism()`. On Linux that is the
//! process's affinity mask, lowered to a cgroup CPU quota where one is set
//! (`cpu.max` on cgroup v2, `cpu.cfs_quota_us` on v1), rounded down and never
//! below one. A cgroup weight (`cpu.weight`, `cpu.shares`) is not a limit and
//! is not read, and a limit a sandbox enforces outside the container is not
//! visible to it at all; a container held back only in one of those ways sizes
//! itself from every CPU it can see.
//!
//! std does not report which input set the number. Its result is the smaller
//! of the affinity count and the quota, so the source is read back by
//! comparing the result with the affinity and online counts, without parsing
//! cgroups a second time.

/// What set [`CpuCount::cpus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CpuSource {
    /// Neither an affinity mask nor a quota lowered it: every online CPU.
    Host,
    /// The affinity mask allows fewer CPUs than are online.
    Affinity,
    /// A cgroup CPU quota is below the affinity count.
    CgroupQuota,
    /// std could not count CPUs and the count is the fallback, or the affinity
    /// mask or the online count could not be read to compare against.
    Unknown,
}

impl CpuSource {
    pub fn as_str(self) -> &'static str {
        match self {
            CpuSource::Host => "host",
            CpuSource::Affinity => "affinity",
            CpuSource::CgroupQuota => "cgroup quota",
            CpuSource::Unknown => "unknown",
        }
    }
}

/// The CPU count defaults are sized from, with what set it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuCount {
    pub cpus: usize,
    pub source: CpuSource,
    /// CPUs reported online, which a mask or quota does not lower unless the
    /// runtime rewrites what reports them; `None` when unreadable.
    pub online: Option<usize>,
}

/// Count used when std cannot determine one.
const FALLBACK_CPUS: usize = 4;

pub fn detect() -> CpuCount {
    let online = online_cpus();
    match std::thread::available_parallelism() {
        Ok(n) => {
            let cpus = n.get();
            CpuCount {
                cpus,
                source: classify(cpus, affinity_cpus(), online),
                online,
            }
        }
        Err(_) => CpuCount {
            cpus: FALLBACK_CPUS,
            source: CpuSource::Unknown,
            online,
        },
    }
}

/// Read back which limit set `cpus`. std returns `min(affinity, quota)`, so a
/// result below the mask can only be the quota. Where the mask cannot be read,
/// std counts `sysconf(_SC_NPROCESSORS_ONLN)` instead, which musl answers from
/// that same failed call, so nothing is left to compare against.
fn classify(cpus: usize, affinity: Option<usize>, online: Option<usize>) -> CpuSource {
    match (affinity, online) {
        (Some(a), _) if cpus < a => CpuSource::CgroupQuota,
        (Some(a), Some(o)) if a < o => CpuSource::Affinity,
        (Some(_), Some(_)) => CpuSource::Host,
        _ => CpuSource::Unknown,
    }
}

/// The same mask std counts, on the same targets.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn affinity_cpus() -> Option<usize> {
    // SAFETY: `set` is a zeroed `cpu_set_t` (a plain bit array) owned by this
    // frame, and the call writes at most `size_of::<cpu_set_t>()` bytes to it.
    let count = unsafe {
        let mut set: libc::cpu_set_t = std::mem::zeroed();
        if libc::sched_getaffinity(0, std::mem::size_of::<libc::cpu_set_t>(), &mut set) != 0 {
            return None;
        }
        libc::CPU_COUNT(&set)
    };
    // std treats an empty mask as unreadable too: some old kernels returned
    // one zeroed when none had been set.
    usize::try_from(count).ok().filter(|&n| n > 0)
}

/// std reads no mask on Apple targets and counts the online CPUs, so every one
/// of them is allowed.
#[cfg(target_vendor = "apple")]
fn affinity_cpus() -> Option<usize> {
    online_cpus()
}

/// Elsewhere std may count a mask this does not read (FreeBSD and NetBSD do),
/// so there is nothing to compare against.
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn affinity_cpus() -> Option<usize> {
    None
}

/// Read from sysfs, not `sysconf(_SC_NPROCESSORS_ONLN)`: musl answers that
/// by counting the affinity mask, which would make a narrowed mask look like
/// the whole machine.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn online_cpus() -> Option<usize> {
    std::fs::read_to_string("/sys/devices/system/cpu/online")
        .ok()
        .and_then(|list| parse_cpu_list(&list))
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
fn online_cpus() -> Option<usize> {
    // SAFETY: `sysconf` takes an integer name and touches no caller memory.
    let n = unsafe { libc::sysconf(libc::_SC_NPROCESSORS_ONLN) };
    usize::try_from(n).ok().filter(|&n| n > 0)
}

/// Count the CPUs in a kernel CPU list (`0-3,8,10-11`); `None` if it does not
/// parse.
#[cfg(any(target_os = "linux", target_os = "android", test))]
fn parse_cpu_list(list: &str) -> Option<usize> {
    let mut count = 0usize;
    for range in list.trim().split(',') {
        let (lo, hi) = match range.split_once('-') {
            Some((lo, hi)) => (lo.parse::<usize>().ok()?, hi.parse::<usize>().ok()?),
            None => {
                let n = range.parse::<usize>().ok()?;
                (n, n)
            }
        };
        count += hi.checked_sub(lo)? + 1;
    }
    Some(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_below_the_mask_is_the_quota() {
        assert_eq!(classify(1, Some(14), Some(14)), CpuSource::CgroupQuota);
        // Below a narrowed mask it is still the quota, not the mask.
        assert_eq!(classify(1, Some(4), Some(14)), CpuSource::CgroupQuota);
        // The mask alone decides; an unreadable online count does not hide it.
        assert_eq!(classify(1, Some(8), None), CpuSource::CgroupQuota);
    }

    #[test]
    fn mask_narrower_than_online_is_affinity() {
        assert_eq!(classify(1, Some(1), Some(14)), CpuSource::Affinity);
    }

    #[test]
    fn nothing_lowered_is_host() {
        assert_eq!(classify(14, Some(14), Some(14)), CpuSource::Host);
    }

    #[test]
    fn unreadable_mask_is_unknown() {
        // musl's sysconf fallback reads 1 when the mask cannot be read; that is
        // not a quota.
        assert_eq!(classify(1, None, Some(14)), CpuSource::Unknown);
        assert_eq!(classify(8, None, Some(8)), CpuSource::Unknown);
    }

    #[test]
    fn no_online_count_and_no_quota_is_unknown() {
        assert_eq!(classify(8, Some(8), None), CpuSource::Unknown);
        assert_eq!(classify(8, None, None), CpuSource::Unknown);
    }

    #[test]
    fn cpu_list_counts_ranges_and_singles() {
        assert_eq!(parse_cpu_list("0-13\n"), Some(14));
        assert_eq!(parse_cpu_list("0"), Some(1));
        assert_eq!(parse_cpu_list("0-3,8,10-11"), Some(7));
    }

    #[test]
    fn cpu_list_that_does_not_parse_is_none() {
        assert_eq!(parse_cpu_list(""), None);
        assert_eq!(parse_cpu_list("3-1"), None);
        assert_eq!(parse_cpu_list("0-x"), None);
    }

    #[test]
    fn detect_reports_the_count_std_returns() {
        let expected = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(FALLBACK_CPUS);
        assert_eq!(detect().cpus, expected);
    }
}
