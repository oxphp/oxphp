//! Timeout wire contract for Shared\* types.
//!
//! All timeout-bearing FFI functions use a single `i64 timeout_ms` wire format:
//!   - `-1` (or any negative value) — wait forever (no timeout)
//!   - `0`                          — non-blocking try (return immediately)
//!   - `> 0`                        — bounded wait of exactly that many milliseconds
//!
//! [`parse_timeout`] converts the wire value to [`Wait`].
//! [`read_timeout_arg`] reads a PHP `?float $timeout = null` argument at a
//! given index and converts it to the wire value, raising `TypeException` on
//! invalid input.

use std::time::Duration;

use crate::bridge::call::NativeCall;
use crate::bridge::types::ValType;
use crate::plugin::PhpError;

/// Decoded timeout instruction.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Wait {
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

/// Read a PHP `?float $timeout = null` argument at position `idx` and return
/// the `i64 timeout_ms` wire value.
///
/// Mapping:
/// - Arg absent or `null`        → `-1` (forever)
/// - `INF`                       → `-1` (forever)
/// - `NaN`                       → `TypeException`
/// - Finite negative             → `TypeException`
/// - `0.0` / `0`                 → `0` (try)
/// - Positive float/int          → `round(secs * 1000)` clamped to `i64::MAX`
/// - Non-numeric type            → `TypeException`
#[allow(dead_code)]
pub(crate) fn read_timeout_arg(call: &NativeCall, idx: u32) -> Result<i64, PhpError> {
    if call.argc() <= idx {
        return Ok(-1);
    }

    let t = call.arg_type(idx)?;

    let secs: f64 = match t {
        ValType::Null => return Ok(-1),
        ValType::Long => call.arg_long(idx)? as f64,
        ValType::Double => call.arg_double(idx)?,
        _ => return Err(type_exception("$timeout must be float|int|null")),
    };

    if secs.is_nan() {
        return Err(type_exception("$timeout must not be NaN"));
    }
    if secs.is_infinite() {
        return Ok(-1);
    }
    if secs < 0.0 {
        return Err(type_exception("$timeout must be non-negative or null"));
    }

    let ms = (secs * 1000.0).round();
    let timeout_ms = if ms >= i64::MAX as f64 {
        i64::MAX
    } else {
        ms as i64
    };

    Ok(timeout_ms)
}

fn type_exception(msg: &str) -> PhpError {
    PhpError::Exception {
        class: "OxPHP\\Shared\\TypeException".into(),
        message: msg.into(),
        code: 0,
    }
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
}
