//! Logs effective uid/gid and supplementary groups at startup.
//!
//! Helps operators verify their privilege-drop config (Docker `--user`, Compose
//! `user:`, Kubernetes `securityContext.runAsUser`) actually took effect, and
//! warns when the server is running as root.

#[cfg(unix)]
#[derive(Debug, Default)]
pub struct Identity {
    pub uid: u32,
    pub gid: u32,
    pub username: Option<String>,
    pub groupname: Option<String>,
    /// Supplementary groups as (gid, name) pairs. Includes the primary gid
    /// because `getgroups()` on Linux returns it; we keep that behavior so
    /// the log mirrors what `id` shows.
    pub supplementary: Vec<(u32, Option<String>)>,
}

#[cfg(unix)]
pub fn collect_identity() -> Identity {
    // SAFETY: geteuid/getegid are async-signal-safe and never fail.
    let uid = unsafe { libc::geteuid() };
    let gid = unsafe { libc::getegid() };

    Identity {
        uid,
        gid,
        username: resolve_username(uid),
        groupname: resolve_groupname(gid),
        supplementary: collect_supplementary_groups(),
    }
}

#[cfg(unix)]
fn resolve_username(uid: u32) -> Option<String> {
    let mut buf = vec![0u8; 1024];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: getpwuid_r writes into `pwd` and `buf` which are owned here;
    // `result` is set to NULL on miss / non-zero return.
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    // SAFETY: pw_name points into `buf` for the lifetime of `pwd`.
    let cstr = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
    cstr.to_str().ok().map(String::from)
}

#[cfg(unix)]
fn resolve_groupname(gid: u32) -> Option<String> {
    let mut buf = vec![0u8; 1024];
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::group = std::ptr::null_mut();
    // SAFETY: same contract as getpwuid_r above.
    let rc = unsafe {
        libc::getgrgid_r(
            gid,
            &mut grp,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    // SAFETY: gr_name points into `buf` for the lifetime of `grp`.
    let cstr = unsafe { std::ffi::CStr::from_ptr(grp.gr_name) };
    cstr.to_str().ok().map(String::from)
}

#[cfg(unix)]
fn collect_supplementary_groups() -> Vec<(u32, Option<String>)> {
    // SAFETY: passing NULL with size 0 returns the actual group count.
    let n = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    if n <= 0 {
        return Vec::new();
    }
    let mut gids = vec![0 as libc::gid_t; n as usize];
    // SAFETY: gids has capacity == n; getgroups writes up to that many.
    let written = unsafe { libc::getgroups(n, gids.as_mut_ptr()) };
    if written <= 0 {
        return Vec::new();
    }
    gids.truncate(written as usize);
    gids.into_iter()
        .map(|g| (g, resolve_groupname(g)))
        .collect()
}

#[cfg(unix)]
fn format_groups(supplementary: &[(u32, Option<String>)]) -> String {
    supplementary
        .iter()
        .map(|(g, name)| match name {
            Some(n) => format!("{g}({n})"),
            None => g.to_string(),
        })
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(unix)]
pub fn log_startup_identity() {
    let id = collect_identity();
    let groups_display = format_groups(&id.supplementary);
    tracing::info!(
        event = "startup_identity",
        uid = id.uid,
        gid = id.gid,
        user = id.username.as_deref().unwrap_or(""),
        group = id.groupname.as_deref().unwrap_or(""),
        groups = %groups_display,
        "starting as uid={}({}) gid={}({}) groups={}",
        id.uid,
        id.username.as_deref().unwrap_or("?"),
        id.gid,
        id.groupname.as_deref().unwrap_or("?"),
        groups_display,
    );

    if id.uid == 0 {
        tracing::warn!(
            event = "running_as_root",
            "running as root — drop privileges via `docker run --user www-data`, Compose `user:`, or Kubernetes `securityContext.runAsUser`"
        );
    }
}

#[cfg(not(unix))]
pub fn log_startup_identity() {
    // No effective uid/gid concept on Windows; nothing to log.
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn collects_current_process_identity() {
        let id = collect_identity();
        // geteuid always succeeds, so uid is set. We can't assert a specific
        // value (CI runs as different users), only that it parses correctly.
        let expected_uid = unsafe { libc::geteuid() };
        let expected_gid = unsafe { libc::getegid() };
        assert_eq!(id.uid, expected_uid);
        assert_eq!(id.gid, expected_gid);
    }

    #[test]
    fn formats_groups_with_and_without_names() {
        let sup = vec![
            (82, Some("www-data".to_string())),
            (101, None),
            (1000, Some("users".to_string())),
        ];
        assert_eq!(format_groups(&sup), "82(www-data),101,1000(users)");
    }

    #[test]
    fn formats_empty_groups() {
        assert_eq!(format_groups(&[]), "");
    }

    #[test]
    fn log_does_not_panic() {
        // Smoke test: just verify the function runs without panicking.
        // We can't easily assert on tracing output without a custom subscriber,
        // and the WARN-when-root branch is trivial enough that visual review
        // of the format covers it.
        log_startup_identity();
    }
}
