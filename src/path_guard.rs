//! Location checks for a file the server opens on a request's behalf.
//!
//! A path that lay inside the document root when the router looked at it can
//! be a link to somewhere else by the time the file is opened, so a verdict
//! reached from the path says nothing about the file that is then read or
//! executed. The checks here read the location from the open descriptor, which
//! names the file that was actually opened.

use std::fs::File;
use std::io;
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::SymlinkAllowList;

/// Where files may be served or executed from: the canonical document root
/// plus the explicitly allowed symlink targets.
#[derive(Debug)]
pub struct PathPolicy {
    canonical_root: PathBuf,
    symlink_allow: SymlinkAllowList,
}

impl PathPolicy {
    pub fn new(canonical_root: PathBuf, symlink_allow: SymlinkAllowList) -> Self {
        Self {
            canonical_root,
            symlink_allow,
        }
    }

    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    pub fn symlink_allow(&self) -> &SymlinkAllowList {
        &self.symlink_allow
    }

    /// Whether a canonical path lies inside the root or an allowed target.
    pub fn allows(&self, canonical_path: &Path) -> bool {
        path_allowed(canonical_path, &self.canonical_root, &self.symlink_allow)
    }
}

pub(crate) fn path_allowed(
    path: &Path,
    canonical_root: &Path,
    allow_list: &SymlinkAllowList,
) -> bool {
    path.starts_with(canonical_root) || allow_list.allows(path)
}

/// How long an open refused by another holder's lease keeps being retried.
/// Linux removes a lease by force after `/proc/sys/fs/lease-break-time`
/// (45 s by default), so the open gets through before this unless that limit
/// was raised; past it the refusal is returned as an error.
pub(crate) const LEASE_WAIT: Duration = Duration::from_secs(50);

/// Path of the file behind `fd`, as the kernel records it.
#[cfg(target_os = "linux")]
pub(crate) fn descriptor_paths(fd: RawFd) -> io::Result<Vec<PathBuf>> {
    std::fs::read_link(format!("/proc/self/fd/{fd}")).map(|path| vec![path])
}

/// Paths of the file behind `fd`, as the kernel records it: with and without
/// firmlinks. `realpath` keeps whichever spelling the configured root used —
/// `/Users/...` or `/System/Volumes/Data/Users/...` — while F_GETPATH always
/// reports the firmlinked one, so a root written through the data volume
/// would otherwise refuse every file under it.
#[cfg(target_os = "macos")]
pub(crate) fn descriptor_paths(fd: RawFd) -> io::Result<Vec<PathBuf>> {
    let mut paths = vec![fcntl_path(fd, libc::F_GETPATH)?];
    if let Ok(path) = fcntl_path(fd, libc::F_GETPATH_NOFIRMLINK) {
        paths.push(path);
    }
    Ok(paths)
}

#[cfg(target_os = "macos")]
fn fcntl_path(fd: RawFd, cmd: libc::c_int) -> io::Result<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH and F_GETPATH_NOFIRMLINK write a NUL-terminated path
    // of at most MAXPATHLEN (== PATH_MAX) bytes into the buffer, which is
    // PATH_MAX bytes long.
    if unsafe { libc::fcntl(fd, cmd, buf.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&buf[..len])))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(crate) fn descriptor_paths(_fd: RawFd) -> io::Result<Vec<PathBuf>> {
    Err(io::ErrorKind::Unsupported.into())
}

/// A script file that has been opened and found inside the allowed locations.
#[derive(Debug)]
pub struct VerifiedScript {
    pub file: File,
    /// The open file's location, as the kernel records it for the descriptor
    /// that was checked. Symlinks in the requested path are resolved in it,
    /// which is the form PHP reports for `__FILE__` when it opens a script
    /// itself. PHP is handed it as the script's name too, so what the engine
    /// derives from that name (its OPcache entry, its working directory) is the
    /// checked file's, not whatever the requested path points at later.
    pub opened_path: PathBuf,
}

#[derive(Debug)]
pub enum ScriptOpenError {
    /// The file that was opened lies outside the root and every allowed
    /// symlink target.
    Escapes,
    /// The file could not be opened.
    Io(io::Error),
}

/// Open a script for execution and check where the opened file lives.
///
/// The check reads the location from the descriptor, so the file it describes
/// is the one the caller then hands to PHP; a link repointed before, during or
/// after the open cannot make the verdict describe a different file. Where the
/// descriptor's path is unavailable (no `/proc`, another unix), the path is
/// resolved again and must both be allowed and name the opened inode — a
/// narrower guarantee. A failed descriptor check does not fall back to that
/// resolution: a file outside the root always fails it, and re-resolving would
/// hand an attacker the fallback's race on demand. The opened file must be a
/// regular file, as for PHP's own open of a script: anything else is
/// `Io` with `InvalidInput`. The returned file is in blocking mode, as the
/// engine's own open of a script leaves it.
///
/// Blocks the calling thread while another process holds a write lease on the
/// file, up to [`LEASE_WAIT`] — as the blocking open this replaces did.
pub fn open_script_verified(
    path: &Path,
    policy: &PathPolicy,
) -> Result<VerifiedScript, ScriptOpenError> {
    open_checked(path, policy, || ())
}

/// [`open_script_verified`] with a hook that runs between the open and the
/// check — the window a swap would use; tests repoint a link in it.
fn open_checked(
    path: &Path,
    policy: &PathPolicy,
    after_open: impl FnOnce(),
) -> Result<VerifiedScript, ScriptOpenError> {
    let file = open_nonblocking(path).map_err(ScriptOpenError::Io)?;
    after_open();
    let opened_path = match descriptor_paths(file.as_raw_fd()) {
        Ok(paths) => paths.into_iter().find(|p| policy.allows(p)),
        Err(_) => resolved_path_naming_open_file(path, &file, policy),
    };
    let Some(opened_path) = opened_path else {
        return Err(ScriptOpenError::Escapes);
    };
    // PHP's own open of a script refuses anything but a regular file; the file
    // handed over here must too, or a directory would read as an empty script
    // and a FIFO as end of file. Checked after the location, so a link
    // repointed outside the root is still reported as escaping.
    if !file.metadata().map_err(ScriptOpenError::Io)?.is_file() {
        return Err(ScriptOpenError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "not a regular file",
        )));
    }
    // It is a regular file now, so the flag has done its work. PHP reads the
    // script from this descriptor, and its stream layer takes EAGAIN for "no
    // data yet", which the engine then reads as the end of the file: a
    // filesystem that answers a non-blocking read with EAGAIN would run a
    // truncated script as a success. PHP opens a script without the flag.
    // SAFETY: `file` owns the descriptor; F_SETFL with 0 only clears the
    // status flags (O_NONBLOCK was the one set, by `open_nonblocking_for`).
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, 0) } == -1 {
        return Err(ScriptOpenError::Io(io::Error::last_os_error()));
    }
    Ok(VerifiedScript { file, opened_path })
}

/// Open `path` read-only with O_NONBLOCK, so a link to a FIFO cannot block the
/// calling thread before the location check refuses it. On Linux the flag also
/// makes the open of a regular file fail with EWOULDBLOCK while another process
/// holds a conflicting write lease (Samba with kernel oplocks does); the open
/// is then retried with the flag still set, never without it — each attempt
/// resolves the path again, and a blocking retry would block for good on a link
/// repointed at a FIFO between the attempts.
fn open_nonblocking(path: &Path) -> io::Result<File> {
    open_nonblocking_for(path, LEASE_WAIT)
}

fn open_nonblocking_for(path: &Path, lease_wait: Duration) -> io::Result<File> {
    let deadline = Instant::now() + lease_wait;
    let mut pause = Duration::from_millis(5);
    loop {
        match File::options()
            .read(true)
            .custom_flags(libc::O_NONBLOCK)
            .open(path)
        {
            Err(e) if e.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                std::thread::sleep(pause);
                pause = (pause * 2).min(Duration::from_millis(100));
            }
            other => return other,
        }
    }
}

/// Fallback for [`open_script_verified`]: resolve `path` again and require the
/// result to be allowed and to name the same inode as the open file. Narrower
/// than the descriptor check — a directory swapped back and forth between the
/// resolution and its `stat` can still pass — but a link swapped once, before
/// or after the open, is refused.
fn resolved_path_naming_open_file(
    path: &Path,
    file: &File,
    policy: &PathPolicy,
) -> Option<PathBuf> {
    let real = std::fs::canonicalize(path).ok()?;
    if !policy.allows(&real) {
        return None;
    }
    let opened = file.metadata().ok()?;
    let named = std::fs::metadata(&real).ok()?;
    (named.dev() == opened.dev() && named.ino() == opened.ino()).then_some(real)
}

/// A policy for tests that build a `ScriptRequest` and never open its script.
#[cfg(test)]
pub(crate) fn test_policy(root: &str) -> std::sync::Arc<PathPolicy> {
    std::sync::Arc::new(PathPolicy::new(
        PathBuf::from(root),
        SymlinkAllowList::default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::symlink_allow::tests::{non_blacklisted_tempdir, with_env};
    use std::os::unix::fs::symlink;

    /// A document root and a sibling directory outside it, both canonical.
    struct Layout {
        _dir: tempfile::TempDir,
        root: PathBuf,
        outside: PathBuf,
    }

    fn layout() -> Layout {
        let dir = non_blacklisted_tempdir();
        let base = std::fs::canonicalize(dir.path()).unwrap();
        let root = base.join("public");
        let outside = base.join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        Layout {
            _dir: dir,
            root,
            outside,
        }
    }

    fn policy(l: &Layout) -> PathPolicy {
        PathPolicy::new(l.root.clone(), SymlinkAllowList::default())
    }

    fn opened(result: Result<VerifiedScript, ScriptOpenError>) -> PathBuf {
        match result {
            Ok(script) => script.opened_path,
            Err(e) => panic!("expected the script to open, got {e:?}"),
        }
    }

    fn escapes(result: Result<VerifiedScript, ScriptOpenError>) -> bool {
        matches!(result, Err(ScriptOpenError::Escapes))
    }

    #[test]
    fn regular_file_inside_the_root_opens() {
        let l = layout();
        let script = l.root.join("inside.php");
        std::fs::write(&script, "<?php echo 'in';").unwrap();

        assert_eq!(opened(open_script_verified(&script, &policy(&l))), script);
    }

    #[test]
    fn the_opened_script_is_handed_over_in_blocking_mode() {
        let l = layout();
        let script = l.root.join("inside.php");
        std::fs::write(&script, "<?php echo 'in';").unwrap();

        let verified = open_script_verified(&script, &policy(&l)).unwrap();

        // SAFETY: the descriptor belongs to `verified.file`, alive here.
        let flags = unsafe { libc::fcntl(verified.file.as_raw_fd(), libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(flags & libc::O_NONBLOCK, 0);
    }

    #[test]
    fn link_inside_the_root_reports_the_resolved_location() {
        let l = layout();
        let target = l.root.join("inside.php");
        std::fs::write(&target, "<?php echo 'in';").unwrap();
        let link = l.root.join("x.php");
        symlink(&target, &link).unwrap();

        // `__FILE__` of a script reached through a link is its resolved path;
        // the location handed to PHP must keep that.
        assert_eq!(opened(open_script_verified(&link, &policy(&l))), target);
    }

    #[test]
    fn link_to_a_file_outside_the_root_is_refused() {
        let l = layout();
        std::fs::write(l.outside.join("secret.php"), "<?php echo 'out';").unwrap();
        let link = l.root.join("x.php");
        symlink(l.outside.join("secret.php"), &link).unwrap();

        assert!(escapes(open_script_verified(&link, &policy(&l))));
    }

    #[test]
    fn link_repointed_outside_after_it_was_accepted_is_refused() {
        let l = layout();
        let inside = l.root.join("inside.php");
        std::fs::write(&inside, "<?php echo 'in';").unwrap();
        let secret = l.outside.join("secret.php");
        std::fs::write(&secret, "<?php echo 'out';").unwrap();
        let link = l.root.join("x.php");
        symlink(&inside, &link).unwrap();
        let policy = policy(&l);

        assert_eq!(opened(open_script_verified(&link, &policy)), inside);

        // The routing layer's verdict for `link` is cached for good; the check
        // at the open must not be.
        std::fs::remove_file(&link).unwrap();
        symlink(&secret, &link).unwrap();
        assert!(escapes(open_script_verified(&link, &policy)));
    }

    #[test]
    fn directory_link_repointed_outside_is_refused() {
        let l = layout();
        let inside_dir = l.root.join("real");
        std::fs::create_dir(&inside_dir).unwrap();
        std::fs::write(inside_dir.join("x.php"), "<?php echo 'in';").unwrap();
        std::fs::write(l.outside.join("x.php"), "<?php echo 'out';").unwrap();
        let dir_link = l.root.join("dir");
        symlink(&inside_dir, &dir_link).unwrap();
        let script = dir_link.join("x.php");
        let policy = policy(&l);

        assert_eq!(
            opened(open_script_verified(&script, &policy)),
            inside_dir.join("x.php")
        );

        std::fs::remove_file(&dir_link).unwrap();
        symlink(&l.outside, &dir_link).unwrap();
        assert!(escapes(open_script_verified(&script, &policy)));
    }

    #[test]
    fn link_to_another_file_inside_the_root_still_opens() {
        let l = layout();
        let a = l.root.join("a.php");
        let b = l.root.join("b.php");
        std::fs::write(&a, "<?php echo 'a';").unwrap();
        std::fs::write(&b, "<?php echo 'b';").unwrap();
        let link = l.root.join("x.php");
        symlink(&a, &link).unwrap();
        let policy = policy(&l);
        assert_eq!(opened(open_script_verified(&link, &policy)), a);

        std::fs::remove_file(&link).unwrap();
        symlink(&b, &link).unwrap();
        assert_eq!(opened(open_script_verified(&link, &policy)), b);
    }

    #[test]
    fn allow_listed_target_outside_the_root_opens() {
        let l = layout();
        let shared = l.outside.join("shared");
        std::fs::create_dir(&shared).unwrap();
        let target = shared.join("lib.php");
        std::fs::write(&target, "<?php echo 'shared';").unwrap();
        let link = l.root.join("lib.php");
        symlink(&target, &link).unwrap();
        // Another allowed directory must not admit this one.
        let elsewhere = l.outside.join("elsewhere");
        std::fs::create_dir(&elsewhere).unwrap();

        with_env(Some(shared.to_str().unwrap()), || {
            let allowed =
                PathPolicy::new(l.root.clone(), SymlinkAllowList::from_env(&l.root).unwrap());
            assert_eq!(opened(open_script_verified(&link, &allowed)), target);
        });
        with_env(Some(elsewhere.to_str().unwrap()), || {
            let other =
                PathPolicy::new(l.root.clone(), SymlinkAllowList::from_env(&l.root).unwrap());
            assert!(escapes(open_script_verified(&link, &other)));
        });
    }

    #[test]
    fn missing_file_is_an_io_error_not_an_escape() {
        let l = layout();
        let missing = l.root.join("gone.php");

        assert!(matches!(
            open_script_verified(&missing, &policy(&l)),
            Err(ScriptOpenError::Io(e)) if e.kind() == io::ErrorKind::NotFound
        ));
    }

    #[test]
    fn link_to_a_fifo_outside_the_root_is_refused_without_blocking() {
        use std::os::unix::ffi::OsStrExt;
        let l = layout();
        let fifo = l.outside.join("pipe");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c_path` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let link = l.root.join("x.php");
        symlink(&fifo, &link).unwrap();

        // A blocking open of a FIFO with no writer never returns, so the open
        // runs on its own thread and the test gives up waiting for it.
        let policy = policy(&l);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(escapes(open_script_verified(&link, &policy)));
        });
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(refused) => assert!(refused),
            Err(_) => panic!("open of a link to a FIFO blocked"),
        }
    }

    fn is_not_a_regular_file(result: Result<VerifiedScript, ScriptOpenError>) -> bool {
        matches!(result, Err(ScriptOpenError::Io(e)) if e.kind() == io::ErrorKind::InvalidInput)
    }

    #[test]
    fn link_to_a_directory_inside_the_root_is_not_a_script() {
        let l = layout();
        let dir = l.root.join("real");
        std::fs::create_dir(&dir).unwrap();
        let link = l.root.join("x.php");
        symlink(&dir, &link).unwrap();

        assert!(is_not_a_regular_file(open_script_verified(
            &link,
            &policy(&l)
        )));
    }

    #[test]
    fn link_to_a_fifo_inside_the_root_is_not_a_script_and_does_not_block() {
        use std::os::unix::ffi::OsStrExt;
        let l = layout();
        let fifo = l.root.join("pipe");
        let c_path = std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
        // SAFETY: `c_path` is a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);
        let link = l.root.join("x.php");
        symlink(&fifo, &link).unwrap();

        let policy = policy(&l);
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(is_not_a_regular_file(open_script_verified(&link, &policy)));
        });
        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(refused) => assert!(refused),
            Err(_) => panic!("open of a link to a FIFO inside the root blocked"),
        }
    }

    /// Holds a write lease on `path` until dropped. The kernel signals the
    /// holder when another open wants the file, and the signal's default action
    /// ends the process.
    #[cfg(target_os = "linux")]
    struct WriteLease(File);

    #[cfg(target_os = "linux")]
    impl WriteLease {
        fn take(path: &Path) -> Self {
            // SAFETY: setting a signal's disposition to ignore.
            unsafe { libc::signal(libc::SIGIO, libc::SIG_IGN) };
            let file = File::open(path).unwrap();
            // SAFETY: `file` is an open descriptor.
            let rc = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETLEASE, libc::F_WRLCK) };
            assert_eq!(rc, 0, "{}", io::Error::last_os_error());
            Self(file)
        }
    }

    #[cfg(target_os = "linux")]
    impl Drop for WriteLease {
        fn drop(&mut self) {
            // SAFETY: `self.0` is an open descriptor.
            unsafe { libc::fcntl(self.0.as_raw_fd(), libc::F_SETLEASE, libc::F_UNLCK) };
        }
    }

    // A conflicting write lease makes an O_NONBLOCK open fail with EWOULDBLOCK.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_open_refused_by_a_write_lease_is_retried_until_the_lease_goes() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let script = root.join("leased.php");
        std::fs::write(&script, "<?php echo 'in';").unwrap();
        let lease = WriteLease::take(&script);
        let policy = PathPolicy::new(root, SymlinkAllowList::default());

        let path = script.clone();
        let (running_tx, running_rx) = std::sync::mpsc::channel();
        let opener = std::thread::spawn(move || {
            let started = Instant::now();
            running_tx.send(()).unwrap();
            (
                open_script_verified(&path, &policy).is_ok(),
                started.elapsed(),
            )
        });
        // The clock starts before the signal and the lease is released after
        // it, so an open that returns while the lease is still held cannot
        // have waited as long as the pause below.
        running_rx.recv().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        drop(lease);
        let (opened, waited) = opener.join().unwrap();

        assert!(opened);
        assert!(
            waited >= Duration::from_millis(300),
            "returned before the lease was released: {waited:?}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_open_refused_by_a_write_lease_gives_up_after_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("leased.php");
        std::fs::write(&script, "<?php echo 'in';").unwrap();
        let lease = WriteLease::take(&script);

        let path = script.clone();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let started = Instant::now();
            let result = open_nonblocking_for(&path, Duration::from_millis(200));
            let _ = tx.send((result, started.elapsed()));
        });
        // The lease is held until the opener has answered, so only the wait can
        // end the retries; one that never ends fails here rather than hangs.
        let (result, waited) = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the open kept retrying past its wait");
        drop(lease);

        assert!(
            matches!(&result, Err(e) if e.kind() == io::ErrorKind::WouldBlock),
            "{result:?} after {waited:?}"
        );
        assert!(waited >= Duration::from_millis(200), "{waited:?}");
    }

    #[test]
    fn link_repointed_between_the_open_and_the_check_is_judged_by_the_opened_file() {
        use std::io::Read;
        let l = layout();
        let inside = l.root.join("inside.php");
        std::fs::write(&inside, "<?php echo 'in';").unwrap();
        let secret = l.outside.join("secret.php");
        std::fs::write(&secret, "<?php echo 'out';").unwrap();
        let link = l.root.join("x.php");
        symlink(&inside, &link).unwrap();

        // The link is repointed outside once the file is open. What runs is
        // the file that was opened, and that one is inside.
        let mut script = open_checked(&link, &policy(&l), || {
            std::fs::remove_file(&link).unwrap();
            symlink(&secret, &link).unwrap();
        })
        .unwrap();
        assert_eq!(script.opened_path, inside);
        let mut body = String::new();
        script.file.read_to_string(&mut body).unwrap();
        assert_eq!(body, "<?php echo 'in';");
    }

    // A hard link gives one inode two names, and only Linux reports the name
    // an open used; macOS reports either.
    #[cfg(target_os = "linux")]
    #[test]
    fn an_opened_outside_file_stays_refused_when_the_link_then_resolves_inside() {
        let l = layout();
        let secret = l.outside.join("secret.php");
        std::fs::write(&secret, "<?php echo 'out';").unwrap();
        let alias = l.root.join("alias.php");
        std::fs::hard_link(&secret, &alias).unwrap();
        let link = l.root.join("x.php");
        symlink(&secret, &link).unwrap();

        // The file was opened through the outside name. Repointing the link at
        // its inside name afterwards must not turn the verdict: resolving the
        // path again would now be allowed and name the same inode.
        let result = open_checked(&link, &policy(&l), || {
            std::fs::remove_file(&link).unwrap();
            symlink(&alias, &link).unwrap();
        });
        assert!(escapes(result));
    }

    #[test]
    fn fallback_refuses_a_link_that_names_another_file_than_the_open_one() {
        let l = layout();
        let inside = l.root.join("inside.php");
        std::fs::write(&inside, "<?php echo 'in';").unwrap();
        let other = l.root.join("other.php");
        std::fs::write(&other, "<?php echo 'other';").unwrap();
        let link = l.root.join("x.php");
        symlink(&inside, &link).unwrap();
        let policy = policy(&l);

        // Open through the link, then repoint it at a different file inside
        // the root: the resolution is allowed but names another inode.
        let file = open_nonblocking(&link).unwrap();
        std::fs::remove_file(&link).unwrap();
        symlink(&other, &link).unwrap();
        assert!(resolved_path_naming_open_file(&link, &file, &policy).is_none());

        // And the file that was opened, resolved unchanged, is accepted.
        let same = open_nonblocking(&other).unwrap();
        assert_eq!(
            resolved_path_naming_open_file(&other, &same, &policy),
            Some(other)
        );
    }

    #[test]
    fn fallback_refuses_a_resolution_outside_the_root() {
        let l = layout();
        let secret = l.outside.join("secret.php");
        std::fs::write(&secret, "<?php echo 'out';").unwrap();
        let link = l.root.join("x.php");
        symlink(&secret, &link).unwrap();

        let file = open_nonblocking(&link).unwrap();
        assert!(resolved_path_naming_open_file(&link, &file, &policy(&l)).is_none());
    }
}
