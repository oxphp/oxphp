//! Strict boolean env-var parser shared by core config and plugins.
//!
//! Single source of truth for "is this env var truthy?" so `ENABLED=yes`
//! behaves the same in core and in every plugin. Garbage values are
//! rejected with a startup error rather than silently falling back —
//! catches typos like `WORKER_MODE_ENABLED=ture`.

use crate::types::BoxError;

/// Canonical truthy values (case-insensitive, trimmed).
const BOOL_TRUTHY: &[&str] = &["on", "true", "1", "yes"];

/// Canonical falsy values (case-insensitive, trimmed).
const BOOL_FALSY: &[&str] = &["off", "false", "0", "no"];

/// Strictly parse a boolean-ish string. Accepts [`BOOL_TRUTHY`] → `true` and
/// [`BOOL_FALSY`] → `false` (case-insensitive, trimmed). Empty string and
/// arbitrary garbage are rejected. The returned message lists the accepted
/// values so the operator does not have to guess.
pub(crate) fn parse_bool_strict(value: &str) -> Result<bool, String> {
    let trimmed = value.trim();
    if BOOL_TRUTHY.iter().any(|t| trimmed.eq_ignore_ascii_case(t)) {
        Ok(true)
    } else if BOOL_FALSY.iter().any(|f| trimmed.eq_ignore_ascii_case(f)) {
        Ok(false)
    } else {
        Err(format!(
            "expected one of {} (truthy) or {} (falsy) — case-insensitive, got {value:?}",
            BOOL_TRUTHY.join("/"),
            BOOL_FALSY.join("/"),
        ))
    }
}

/// Read an env var as boolean. Unset or empty (`FOO=`) → `default`; non-empty
/// → strictly parsed via [`parse_bool_strict`]; invalid value bubbles up as a
/// startup error tagged with the variable name.
///
/// Empty is treated as unset on purpose: Docker Compose / Kubernetes
/// substitution like `FOO=${FOO}` produces `FOO=` when the host var is
/// missing, and that should fall back to the default rather than refusing to
/// start.
pub(crate) fn parse_env_bool(name: &str, default: bool) -> Result<bool, BoxError> {
    match std::env::var(name) {
        Ok(value) if !value.trim().is_empty() => {
            parse_bool_strict(&value).map_err(|e| -> BoxError { format!("{name}: {e}").into() })
        }
        _ => Ok(default),
    }
}

/// Like [`parse_env_bool`] but takes a pre-resolved `Option<&str>` (for
/// callers that have their own lookup chain, e.g. plugins that try
/// `PLUGIN_FOO` then `OX_FOO` then `FOO`). `None` and `Some("")` collapse to
/// `default`; any non-empty value is strictly parsed.
#[allow(dead_code)] // consumed by feature-gated plugins
pub(crate) fn parse_bool_opt(
    name: &str,
    value: Option<&str>,
    default: bool,
) -> Result<bool, BoxError> {
    match value {
        Some(v) if !v.trim().is_empty() => {
            parse_bool_strict(v).map_err(|e| -> BoxError { format!("{name}: {e}").into() })
        }
        _ => Ok(default),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_accepts_truthy_set() {
        for val in [
            "on", "true", "1", "yes", "ON", "True", "  yes  ", "\tTRUE\n",
        ] {
            assert_eq!(
                parse_bool_strict(val),
                Ok(true),
                "{val:?} should parse as true"
            );
        }
    }

    #[test]
    fn strict_accepts_falsy_set() {
        for val in ["off", "false", "0", "no", "OFF", "False", "  no  "] {
            assert_eq!(
                parse_bool_strict(val),
                Ok(false),
                "{val:?} should parse as false"
            );
        }
    }

    #[test]
    fn strict_rejects_garbage() {
        for val in ["", "  ", "garbage", "tru", "flase", "y", "n", "2"] {
            let res = parse_bool_strict(val);
            assert!(res.is_err(), "{val:?} should be rejected");
            let msg = res.unwrap_err();
            assert!(
                msg.contains("expected one of"),
                "error should explain accepted values, got: {msg}"
            );
            assert!(
                msg.contains("case-insensitive"),
                "error should hint case-insensitive, got: {msg}"
            );
        }
    }

    #[test]
    fn opt_unset_uses_default() {
        assert!(parse_bool_opt("X", None, true).unwrap());
        assert!(!parse_bool_opt("X", None, false).unwrap());
    }

    #[test]
    fn opt_empty_uses_default() {
        assert!(parse_bool_opt("X", Some(""), true).unwrap());
        assert!(parse_bool_opt("X", Some("   "), true).unwrap());
    }

    #[test]
    fn opt_explicit_overrides_default() {
        assert!(parse_bool_opt("X", Some("yes"), false).unwrap());
        assert!(!parse_bool_opt("X", Some("no"), true).unwrap());
    }

    #[test]
    fn opt_garbage_errors_with_var_name() {
        let err = parse_bool_opt("MY_VAR", Some("garbage"), false).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("MY_VAR"), "msg: {msg}");
        assert!(msg.contains("garbage"), "msg: {msg}");
    }

    fn with_env<F: FnOnce()>(name: &str, value: Option<&str>, f: F) {
        crate::config::test_env::with_env(&[(name, value)], f);
    }

    #[test]
    fn env_unset_uses_default() {
        with_env("OXPHP_TEST_BOOL_UNSET", None, || {
            assert!(parse_env_bool("OXPHP_TEST_BOOL_UNSET", true).unwrap());
            assert!(!parse_env_bool("OXPHP_TEST_BOOL_UNSET", false).unwrap());
        });
    }

    #[test]
    fn env_empty_uses_default() {
        // Docker Compose `FOO=${FOO}` with unset host var produces `FOO=` —
        // must not refuse to start.
        with_env("OXPHP_TEST_BOOL_EMPTY", Some(""), || {
            assert!(parse_env_bool("OXPHP_TEST_BOOL_EMPTY", true).unwrap());
            assert!(!parse_env_bool("OXPHP_TEST_BOOL_EMPTY", false).unwrap());
        });
        with_env("OXPHP_TEST_BOOL_BLANK", Some("   "), || {
            assert!(parse_env_bool("OXPHP_TEST_BOOL_BLANK", true).unwrap());
        });
    }

    #[test]
    fn env_explicit_overrides_default() {
        with_env("OXPHP_TEST_BOOL_SET", Some("yes"), || {
            assert!(parse_env_bool("OXPHP_TEST_BOOL_SET", false).unwrap());
        });
        with_env("OXPHP_TEST_BOOL_SET", Some("off"), || {
            assert!(!parse_env_bool("OXPHP_TEST_BOOL_SET", true).unwrap());
        });
    }

    #[test]
    fn env_garbage_errors_with_var_name() {
        with_env("OXPHP_TEST_BOOL_GARBAGE", Some("ture"), || {
            let err = parse_env_bool("OXPHP_TEST_BOOL_GARBAGE", false).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains("OXPHP_TEST_BOOL_GARBAGE"), "msg: {msg}");
            assert!(msg.contains("ture"), "msg: {msg}");
        });
    }
}
