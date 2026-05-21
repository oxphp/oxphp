//! Timeout wire contract for Shared\* types.
//!
//! All timeout-bearing FFI functions use a single `i64 timeout_ms` wire format:
//!   - `-1` (or any negative value) — wait forever (no timeout)
//!   - `0`                          — non-blocking try (return immediately)
//!   - `> 0`                        — bounded wait of exactly that many milliseconds
//!
//! [`parse_timeout`] converts the wire value to [`Wait`].
//! [`read_positive_ms_arg`] reads a PHP `int $ms` argument (`> 0` required),
//! used by every bounded-wait `*Timeout` method on Shared\Channel,
//! Shared\Mutex and Shared\Pool. Raises `TypeException` on absent / zero /
//! negative / non-int input. There is no float timeout path.

use std::time::Duration;

use crate::bridge::call::NativeCall;
use crate::bridge::types::ValType;
use crate::plugin::PhpError;

/// Decoded timeout instruction.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Wait {
    /// Wait indefinitely (wire: any negative value).
    Forever,
    /// Non-blocking try (wire: 0).
    Try,
    /// Bounded wait of the given duration (wire: positive ms).
    Bounded(Duration),
}

/// Convert the `i64 timeout_ms` wire value to a [`Wait`].
#[allow(dead_code)]
pub(crate) fn parse_timeout(timeout_ms: i64) -> Wait {
    if timeout_ms < 0 {
        Wait::Forever
    } else if timeout_ms == 0 {
        Wait::Try
    } else {
        Wait::Bounded(Duration::from_millis(timeout_ms as u64))
    }
}

fn type_exception(msg: &str) -> PhpError {
    PhpError::Exception {
        class: "OxPHP\\Shared\\TypeException".into(),
        message: msg.into(),
        code: 0,
    }
}

/// Read a PHP `int $ms` argument at position `idx` and return it as the
/// `i64 timeout_ms` wire value. Requires `$ms > 0`.
///
/// Used by all `*Timeout` methods on Shared\Channel and Shared\Mutex.
/// The non-blocking and forever variants are different methods and do
/// not read this argument.
///
/// Mapping:
/// - Long `> 0`              → that value, returned verbatim
/// - Long `<= 0` (incl. 0)   → TypeException "$ms must be > 0"
/// - Any non-Long PHP type   → TypeException "$ms must be int"
/// - Arg absent              → TypeException "$ms is required"
#[allow(dead_code)]
pub(crate) fn read_positive_ms_arg(call: &NativeCall, idx: u32) -> Result<i64, PhpError> {
    if call.argc() <= idx {
        return Err(type_exception("$ms is required"));
    }
    match call.arg_type(idx)? {
        ValType::Long => read_positive_ms_from_long(call.arg_long(idx)?),
        _ => Err(type_exception("$ms must be int")),
    }
}

/// Pure helper for unit testing — production code goes via
/// [`read_positive_ms_arg`] which extracts the long from a [`NativeCall`].
#[allow(dead_code)]
pub(crate) fn read_positive_ms_from_long(ms: i64) -> Result<i64, PhpError> {
    if ms <= 0 {
        return Err(type_exception("$ms must be > 0"));
    }
    Ok(ms)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_negative_is_forever() {
        assert_eq!(parse_timeout(-1), Wait::Forever);
        assert_eq!(parse_timeout(-42), Wait::Forever);
        assert_eq!(parse_timeout(i64::MIN), Wait::Forever);
    }

    #[test]
    fn parse_zero_is_try() {
        assert_eq!(parse_timeout(0), Wait::Try);
    }

    #[test]
    fn parse_positive_is_bounded() {
        assert_eq!(parse_timeout(1), Wait::Bounded(Duration::from_millis(1)));
        assert_eq!(
            parse_timeout(1500),
            Wait::Bounded(Duration::from_millis(1500))
        );
    }

    #[test]
    fn positive_ms_accepts_positive_int() {
        assert_eq!(read_positive_ms_from_long(1).unwrap(), 1);
        assert_eq!(read_positive_ms_from_long(1500).unwrap(), 1500);
        assert_eq!(read_positive_ms_from_long(i64::MAX).unwrap(), i64::MAX);
    }

    #[test]
    fn positive_ms_rejects_zero() {
        let err = read_positive_ms_from_long(0).unwrap_err();
        match err {
            PhpError::Exception { class, .. } => {
                assert_eq!(class, "OxPHP\\Shared\\TypeException");
            }
            other => panic!("expected TypeException, got {other:?}"),
        }
    }

    #[test]
    fn positive_ms_rejects_negative() {
        let err = read_positive_ms_from_long(-1).unwrap_err();
        match err {
            PhpError::Exception { class, .. } => {
                assert_eq!(class, "OxPHP\\Shared\\TypeException");
            }
            other => panic!("expected TypeException, got {other:?}"),
        }
        let err2 = read_positive_ms_from_long(i64::MIN).unwrap_err();
        match err2 {
            PhpError::Exception { class, .. } => {
                assert_eq!(class, "OxPHP\\Shared\\TypeException");
            }
            other => panic!("expected TypeException, got {other:?}"),
        }
    }
}
