//! Normalize the terminal PHP error of a failed request into a `CapturedException`.
//!
//! Two shapes feed in:
//! * Traditional/Framework/SPA — `oxphp_error_cb` records the engine's uncaught
//!   fatal as `E_ERROR` with message `Uncaught <Class>: <msg> in <file>:<line>\n
//!   Stack trace:\n<trace>\n  thrown`. Parsed here.
//! * Worker — the fiber catch site pre-fills `exception_class`/`stacktrace`
//!   structurally; used directly, no parsing.

use crate::types::{CapturedException, PhpScriptError};

/// Scan a request's error stream for the terminal failure (the last
/// `level == "error"` entry) and normalize it. `None` if there is none.
pub fn extract_unhandled_exception(errors: &[PhpScriptError]) -> Option<CapturedException> {
    let err = errors.iter().rev().find(|e| e.level == "error")?;

    let file = (!err.file.is_empty()).then(|| err.file.clone());
    let line = (err.line != 0).then_some(err.line);

    // Worker path pre-fills the class structurally — use it verbatim.
    if let Some(class) = &err.exception_class {
        return Some(CapturedException {
            exception_type: class.clone(),
            message: (!err.message.is_empty()).then(|| err.message.clone()),
            stacktrace: err.stacktrace.clone(),
            file,
            line,
        });
    }

    // Traditional path: parse the engine's "Uncaught …" fatal message.
    if let Some((class, message, stacktrace)) =
        parse_uncaught(&err.message, err.file.as_str(), err.line)
    {
        return Some(CapturedException {
            exception_type: class,
            message,
            stacktrace,
            file,
            line,
        });
    }

    // Plain fatal (undefined function, E_PARSE, …): no Throwable, no trace.
    Some(CapturedException {
        exception_type: err.error_type.to_string(),
        message: (!err.message.is_empty()).then(|| err.message.clone()),
        stacktrace: None,
        file,
        line,
    })
}

/// Parse an `Uncaught <Class>[: <message>] in <file>:<line>\nStack trace:\n<trace>\n  thrown`
/// message. `file`/`line` are the structurally-known origin, used to strip the
/// ` in <file>:<line>` header tail robustly. Returns `None` if not an uncaught
/// message (caller falls back to plain-fatal handling).
fn parse_uncaught(
    msg: &str,
    file: &str,
    line: u32,
) -> Option<(String, Option<String>, Option<String>)> {
    let body = msg.strip_prefix("Uncaught ")?;
    let body = body.strip_suffix("\n  thrown").unwrap_or(body);

    // Split the header from the stack-trace section at the FIRST occurrence, so
    // chained exceptions keep the outermost header and the whole chain in trace.
    let (header, stacktrace) = match body.split_once("\nStack trace:\n") {
        Some((h, t)) => (h, Some(t.trim_end().to_string())),
        None => (body, None),
    };

    // Strip the " in <file>:<line>" tail using the known origin, isolating
    // "<Class>[: <message>]". Robust even if the message itself contains " in ".
    let header = if !file.is_empty() && line != 0 {
        let tail = format!(" in {file}:{line}");
        header.strip_suffix(&tail).unwrap_or(header)
    } else {
        header
    };

    // Class is up to the first ": " (class names never contain ": ").
    let (class, message) = match header.split_once(": ") {
        Some((c, m)) => (c.to_string(), Some(m.to_string())),
        None => (header.to_string(), None),
    };

    Some((class, message, stacktrace))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(
        level: &'static str,
        error_type: &'static str,
        message: &str,
        file: &str,
        line: u32,
    ) -> PhpScriptError {
        PhpScriptError {
            level,
            error_type,
            message: message.into(),
            file: file.into(),
            line,
            stacktrace: None,
            exception_class: None,
        }
    }

    #[test]
    fn simple_uncaught_exception() {
        let msg = "Uncaught RuntimeException: payment failed in /app/pay.php:42\nStack trace:\n#0 /app/pay.php(88): charge()\n#1 {main}\n  thrown";
        let e = err("error", "E_ERROR", msg, "/app/pay.php", 42);
        let c = extract_unhandled_exception(&[e]).unwrap();
        assert_eq!(c.exception_type, "RuntimeException");
        assert_eq!(c.message.as_deref(), Some("payment failed"));
        assert_eq!(c.file.as_deref(), Some("/app/pay.php"));
        assert_eq!(c.line, Some(42));
        assert!(c
            .stacktrace
            .as_deref()
            .unwrap()
            .starts_with("#0 /app/pay.php(88): charge()"));
        assert!(c.stacktrace.as_deref().unwrap().ends_with("{main}"));
    }

    #[test]
    fn message_with_colons() {
        let msg = "Uncaught PDOException: SQLSTATE[HY000]: general error in /db.php:10\nStack trace:\n#0 {main}\n  thrown";
        let c =
            extract_unhandled_exception(&[err("error", "E_ERROR", msg, "/db.php", 10)]).unwrap();
        assert_eq!(c.exception_type, "PDOException");
        assert_eq!(c.message.as_deref(), Some("SQLSTATE[HY000]: general error"));
    }

    #[test]
    fn chained_takes_outermost_class_full_trace() {
        let msg = "Uncaught DomainException: outer in /a.php:5\nStack trace:\n#0 {main}\n\nNext RuntimeException: inner in /b.php:9\nStack trace:\n#0 {main}\n  thrown";
        let c = extract_unhandled_exception(&[err("error", "E_ERROR", msg, "/a.php", 5)]).unwrap();
        assert_eq!(c.exception_type, "DomainException");
        assert_eq!(c.message.as_deref(), Some("outer"));
        assert!(c
            .stacktrace
            .as_deref()
            .unwrap()
            .contains("Next RuntimeException: inner"));
    }

    #[test]
    fn empty_message_form() {
        let msg = "Uncaught LogicException in /x.php:3\nStack trace:\n#0 {main}\n  thrown";
        let c = extract_unhandled_exception(&[err("error", "E_ERROR", msg, "/x.php", 3)]).unwrap();
        assert_eq!(c.exception_type, "LogicException");
        assert_eq!(c.message, None);
        assert_eq!(c.line, Some(3));
    }

    #[test]
    fn plain_fatal_no_class_no_trace() {
        let c = extract_unhandled_exception(&[err(
            "error",
            "E_ERROR",
            "Call to undefined function foo()",
            "/x.php",
            7,
        )])
        .unwrap();
        assert_eq!(c.exception_type, "E_ERROR");
        assert_eq!(
            c.message.as_deref(),
            Some("Call to undefined function foo()")
        );
        assert_eq!(c.stacktrace, None);
        assert_eq!(c.file.as_deref(), Some("/x.php"));
        assert_eq!(c.line, Some(7));
    }

    #[test]
    fn worker_prestructured_used_directly() {
        let mut e = err("error", "E_ERROR", "boom", "/w.php", 11);
        e.exception_class = Some("TypeError".into());
        e.stacktrace = Some("#0 /w.php(11): h()\n#1 {main}".into());
        let c = extract_unhandled_exception(&[e]).unwrap();
        assert_eq!(c.exception_type, "TypeError");
        assert_eq!(c.message.as_deref(), Some("boom"));
        assert_eq!(
            c.stacktrace.as_deref(),
            Some("#0 /w.php(11): h()\n#1 {main}")
        );
    }

    #[test]
    fn picks_last_error_ignores_warnings() {
        let errs = vec![
            err("warn", "E_WARNING", "deprecated thing", "/x.php", 1),
            err(
                "error",
                "E_ERROR",
                "Uncaught RuntimeException: boom in /x.php:9\nStack trace:\n#0 {main}\n  thrown",
                "/x.php",
                9,
            ),
        ];
        let c = extract_unhandled_exception(&errs).unwrap();
        assert_eq!(c.exception_type, "RuntimeException");
    }

    #[test]
    fn none_when_no_error_level() {
        assert!(extract_unhandled_exception(&[err("warn", "E_WARNING", "x", "/a", 1)]).is_none());
    }
}
