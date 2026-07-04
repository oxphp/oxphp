use std::fmt;
use std::str::FromStr;

/// Minimum accepted TLS protocol version, configured via `TLS_MIN_VERSION`.
///
/// The TLS stack only implements TLS 1.2 and 1.3, so the floor is a two-value
/// choice: `1.2` (the default — accepts both versions) or `1.3` (rejects
/// TLS 1.2 ClientHellos at the handshake). The rustls mapping lives in
/// `server::tls`; this module owns only parsing and validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMinVersion {
    V12,
    V13,
}

impl TlsMinVersion {
    /// Read `TLS_MIN_VERSION` from the environment. Unset — or empty, per the
    /// codebase convention for `${VAR:-}`-style substitutions — defaults to
    /// `1.2`, which matches the historical behavior (TLS 1.2 + 1.3 accepted).
    ///
    /// Any other invalid value — including non-UTF-8 bytes (handled by
    /// [`super::optional_utf8_env`]) — is a hard error, not a silent
    /// fallback: an operator who typo'd a security floor must be told, not
    /// quietly given a weaker configuration than they asked for. This runs
    /// unconditionally from `Config::from_env`, so the error surfaces at
    /// startup and in `oxphp config --check` even when TLS itself is not
    /// enabled.
    pub fn from_env() -> Result<Self, crate::types::BoxError> {
        match super::optional_utf8_env("TLS_MIN_VERSION")? {
            None => Ok(Self::V12),
            Some(value) => value.parse(),
        }
    }
}

impl FromStr for TlsMinVersion {
    type Err = crate::types::BoxError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "1.2" => Ok(Self::V12),
            "1.3" => Ok(Self::V13),
            other => Err(format!(
                "invalid TLS_MIN_VERSION \"{other}\": expected \"1.2\" or \"1.3\" \
                 (TLS 1.0 and 1.1 are not supported)"
            )
            .into()),
        }
    }
}

impl fmt::Display for TlsMinVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::V12 => "1.2",
            Self::V13 => "1.3",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_env<F: FnOnce()>(value: Option<&std::ffi::OsStr>, f: F) {
        crate::config::test_env::with_env_os(&[("TLS_MIN_VERSION", value)], f);
    }

    fn with_env_str<F: FnOnce()>(value: Option<&str>, f: F) {
        with_env(value.map(std::ffi::OsStr::new), f);
    }

    #[test]
    fn parse_accepts_supported_floors() {
        assert_eq!("1.2".parse::<TlsMinVersion>().unwrap(), TlsMinVersion::V12);
        assert_eq!("1.3".parse::<TlsMinVersion>().unwrap(), TlsMinVersion::V13);
        // Stray whitespace from compose files / shell quoting is tolerated.
        assert_eq!(
            " 1.3 ".parse::<TlsMinVersion>().unwrap(),
            TlsMinVersion::V13
        );
    }

    #[test]
    fn parse_rejects_unsupported_values() {
        for bad in ["1.1", "1.0", "foo", "13", "tls1.3"] {
            let err = bad.parse::<TlsMinVersion>().unwrap_err();
            assert!(
                err.to_string().contains("TLS_MIN_VERSION"),
                "error for {bad:?} should name the env var: {err}"
            );
        }
    }

    #[test]
    fn from_env_defaults_to_v12_when_unset() {
        with_env_str(None, || {
            assert_eq!(TlsMinVersion::from_env().unwrap(), TlsMinVersion::V12);
        });
    }

    #[test]
    fn from_env_empty_is_treated_as_unset() {
        // `${TLS_MIN_VERSION:-}`-style compose/Helm substitution.
        with_env_str(Some(""), || {
            assert_eq!(TlsMinVersion::from_env().unwrap(), TlsMinVersion::V12);
        });
        // Whitespace-only is NOT collapsed to unset: a junk security floor
        // must fail loudly, not silently pick the default.
        with_env_str(Some("  "), || {
            assert!(TlsMinVersion::from_env().is_err());
        });
    }

    #[test]
    fn from_env_reads_v13() {
        with_env_str(Some("1.3"), || {
            assert_eq!(TlsMinVersion::from_env().unwrap(), TlsMinVersion::V13);
        });
    }

    #[test]
    fn from_env_invalid_value_is_a_hard_error() {
        with_env_str(Some("1.1"), || {
            assert!(TlsMinVersion::from_env().is_err());
        });
    }

    #[cfg(unix)]
    #[test]
    fn from_env_non_utf8_is_a_hard_error() {
        use std::os::unix::ffi::OsStrExt;
        // A corrupted env file / secret templating artifact must not silently
        // weaken the floor to the default.
        let bad = std::ffi::OsStr::from_bytes(b"1.\xFF3");
        with_env(Some(bad), || {
            let err = TlsMinVersion::from_env().unwrap_err();
            assert!(
                err.to_string().contains("UTF-8"),
                "error should mention encoding: {err}"
            );
        });
    }

    #[test]
    fn display_round_trips() {
        assert_eq!(TlsMinVersion::V12.to_string(), "1.2");
        assert_eq!(TlsMinVersion::V13.to_string(), "1.3");
    }
}
