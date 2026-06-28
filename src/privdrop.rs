//! In-binary privilege drop for `oxphp serve --user=...`.
//!
//! Binds privileged sockets as root at startup, then permanently drops to a
//! non-root user/group before any request-handling thread is spawned. OxPHP is
//! single-process — "workers" are OS threads — so `setuid` demotes the *entire*
//! process (glibc/musl broadcast it to every live thread). The drop is meant to
//! run while single-threaded (only the logging writer thread may be alive), so
//! the only thing that ever needs root is binding the listener at startup.

use oxphp::cli::{DropIntent, DropTarget};
use oxphp::types::BoxError;

/// Permanently drop to the target user/group. Call this while the process is
/// effectively single-threaded (before spawning workers, the runtime, or the
/// async pool) and as root.
///
/// The order is security-critical: `initgroups → setgid → setuid → verify →
/// no_new_privs`. Using `setuid` (not `seteuid`) drops the real, effective,
/// and saved uids together, so the drop cannot be undone.
#[cfg(unix)]
pub fn drop_to(t: &DropTarget) -> Result<(), BoxError> {
    // SAFETY: geteuid is async-signal-safe and never fails.
    if unsafe { libc::geteuid() } != 0 {
        return Err("--user requires starting as root".into());
    }

    // 1. Supplementary groups FIRST — this needs root and would fail once the
    //    uid is dropped.
    match &t.user {
        // Named user: install its full supplementary group list (nginx-like).
        Some(name) => {
            // SAFETY: `name` is a valid NUL-terminated CString; `gid` is plain.
            // `as _` adapts to the platform's basegroup type (gid_t on Linux,
            // c_int on macOS).
            if unsafe { libc::initgroups(name.as_ptr(), t.gid as _) } != 0 {
                return Err(
                    format!("initgroups failed: {}", std::io::Error::last_os_error()).into(),
                );
            }
        }
        // Bare numeric uid with no passwd entry: there is no list to expand, so
        // restrict the process to just its primary group (least privilege).
        None => {
            let groups = [t.gid];
            // SAFETY: `groups` is a 1-element array matching the given length.
            if unsafe { libc::setgroups(1, groups.as_ptr()) } != 0 {
                return Err(
                    format!("setgroups failed: {}", std::io::Error::last_os_error()).into(),
                );
            }
        }
    }

    // 2. gid before uid.
    // SAFETY: setgid with a plain gid value.
    if unsafe { libc::setgid(t.gid) } != 0 {
        return Err(format!(
            "setgid({}) failed: {}",
            t.gid,
            std::io::Error::last_os_error()
        )
        .into());
    }

    // 3. uid last; setuid drops real+effective+saved → irreversible.
    // SAFETY: setuid with a plain uid value.
    if unsafe { libc::setuid(t.uid) } != 0 {
        return Err(format!(
            "setuid({}) failed: {}",
            t.uid,
            std::io::Error::last_os_error()
        )
        .into());
    }

    // 4. Paranoid verify: after dropping to a non-root uid, regaining root via
    //    setuid(0) MUST fail. If it succeeds the drop did not stick — abort.
    // SAFETY: setuid(0) is a probe; success here is a fatal security failure.
    if t.uid != 0 && unsafe { libc::setuid(0) } == 0 {
        return Err("privilege drop failed: process regained root".into());
    }

    // 5. Defense in depth (Linux only): forbid regaining privileges later via
    //    setuid/file-capability binaries.
    #[cfg(target_os = "linux")]
    // SAFETY: prctl(PR_SET_NO_NEW_PRIVS) takes a single flag argument; the
    // trailing args are ignored. Best-effort — failure is non-fatal.
    unsafe {
        libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0);
    }

    tracing::info!(
        event = "privilege_drop",
        uid = t.uid,
        gid = t.gid,
        user = t.user.as_ref().and_then(|c| c.to_str().ok()).unwrap_or(""),
        "dropped privileges to uid={} gid={}",
        t.uid,
        t.gid,
    );
    Ok(())
}

/// Non-unix stub. `--user` is rejected at parse time on these platforms, but a
/// stub keeps the `main()` call site cross-platform.
#[cfg(not(unix))]
pub fn drop_to(_t: &DropTarget) -> Result<(), BoxError> {
    Err("privilege drop is not supported on this platform".into())
}

/// Default user for the no-`--user` best-effort drop.
#[cfg(unix)]
const DEFAULT_DROP_USER: &str = "www-data";

/// Outcome of evaluating the best-effort default drop.
#[cfg(unix)]
#[derive(Debug, PartialEq, Eq)]
enum DropPlan {
    /// Not root — already running unprivileged; skip.
    Skip,
    /// Root but the default user does not resolve — stay root, warn.
    StayRoot,
    /// Root and the default user resolved — drop to it.
    Drop(DropTarget),
}

/// Pure decision for the default (`www-data`) drop, split out so the branch
/// logic is unit-testable without performing a real `setuid`.
#[cfg(unix)]
fn plan_default_drop(is_root: bool, resolved: Option<DropTarget>) -> DropPlan {
    match (is_root, resolved) {
        (false, _) => DropPlan::Skip,
        (true, None) => DropPlan::StayRoot,
        (true, Some(t)) => DropPlan::Drop(t),
    }
}

/// Apply a [`DropIntent`]. `Explicit` is fail-fast (delegates to `drop_to`).
/// `DefaultWwwData` is best-effort: it resolves `www-data` at call time and
/// never aborts startup — it drops if root and the user exists, stays root with
/// a warning if root but the user is missing, and skips silently when already
/// non-root.
#[cfg(unix)]
pub fn apply_drop(intent: &DropIntent) -> Result<(), BoxError> {
    match intent {
        DropIntent::Explicit(t) => drop_to(t),
        DropIntent::DefaultWwwData => {
            // SAFETY: geteuid is async-signal-safe and never fails.
            let is_root = unsafe { libc::geteuid() } == 0;
            let resolved = DEFAULT_DROP_USER.parse::<DropTarget>().ok();
            match plan_default_drop(is_root, resolved) {
                DropPlan::Skip => {
                    tracing::debug!(
                        event = "privilege_drop_skipped",
                        "not started as root; running as the current user"
                    );
                    Ok(())
                }
                DropPlan::StayRoot => {
                    tracing::warn!(
                        event = "privilege_drop_unavailable",
                        user = DEFAULT_DROP_USER,
                        "running as root: user '{DEFAULT_DROP_USER}' not found; not dropping \
                         privileges. Pass --user=<name|uid> or run as a non-root user.",
                    );
                    Ok(())
                }
                DropPlan::Drop(t) => drop_to(&t),
            }
        }
    }
}

/// Non-unix stub. Explicit `--user` is rejected at parse time on these
/// platforms; the default intent is a no-op.
#[cfg(not(unix))]
pub fn apply_drop(intent: &DropIntent) -> Result<(), BoxError> {
    match intent {
        DropIntent::Explicit(t) => drop_to(t),
        DropIntent::DefaultWwwData => Ok(()),
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn rejects_when_not_root() {
        // Only meaningful as a non-root process. If the test runner happens to
        // be root we must NOT actually drop — that would demote the test
        // process — so skip; the real drop path is covered by a root-capable
        // integration environment, not the host unit suite.
        // SAFETY: geteuid never fails.
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let target = DropTarget {
            uid: 12345,
            gid: 12345,
            user: None,
        };
        let err = drop_to(&target).expect_err("non-root drop must fail, never silently no-op");
        assert!(err.to_string().contains("root"));
    }

    fn sample_target() -> DropTarget {
        DropTarget {
            uid: 82,
            gid: 82,
            user: None,
        }
    }

    #[test]
    fn plan_nonroot_always_skips() {
        assert_eq!(plan_default_drop(false, None), DropPlan::Skip);
        assert_eq!(
            plan_default_drop(false, Some(sample_target())),
            DropPlan::Skip
        );
    }

    #[test]
    fn plan_root_without_user_stays_root() {
        assert_eq!(plan_default_drop(true, None), DropPlan::StayRoot);
    }

    #[test]
    fn plan_root_with_user_drops() {
        assert_eq!(
            plan_default_drop(true, Some(sample_target())),
            DropPlan::Drop(sample_target())
        );
    }
}
