//! Shared test harness for tests that mutate process-global environment
//! variables. One lock and one save/set/run/restore implementation for the
//! whole crate — module-local copies drifted (some were not panic-safe, some
//! lost non-UTF-8 values on restore).

use std::ffi::{OsStr, OsString};
use std::sync::Mutex;

/// Serializes every env-touching test in the crate. Held internally by
/// [`with_env`]/[`with_env_os`]; RAII-style tests (e.g. `EnvGuard` users)
/// lock it explicitly for their scope instead.
pub(crate) static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Lock, set/unset `vars` in order, run `f` and return its value, restore
/// the previous values (non-UTF-8 safe via `OsString`), then resume any
/// panic — a failed assert cannot leak env state into other tests.
pub(crate) fn with_env_os<R, F: FnOnce() -> R>(vars: &[(&str, Option<&OsStr>)], f: F) -> R {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let prev: Vec<(String, Option<OsString>)> = vars
        .iter()
        .map(|(k, _)| (k.to_string(), std::env::var_os(k)))
        .collect();
    for (k, v) in vars {
        match v {
            Some(val) => std::env::set_var(k, val),
            None => std::env::remove_var(k),
        }
    }
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    for (k, v) in &prev {
        match v {
            Some(val) => std::env::set_var(k, val),
            None => std::env::remove_var(k),
        }
    }
    match result {
        Ok(r) => r,
        Err(e) => std::panic::resume_unwind(e),
    }
}

/// [`with_env_os`] for plain UTF-8 values.
pub(crate) fn with_env<R, F: FnOnce() -> R>(vars: &[(&str, Option<&str>)], f: F) -> R {
    let os_vars: Vec<(&str, Option<&OsStr>)> =
        vars.iter().map(|(k, v)| (*k, v.map(OsStr::new))).collect();
    with_env_os(&os_vars, f)
}
