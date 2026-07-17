//! Normalize the terminal PHP error of a failed request into a `CapturedException`.
//!
//! Two shapes feed in:
//! * Traditional/Framework/SPA — `oxphp_error_cb` records the engine's uncaught
//!   fatal as `E_ERROR` with message `Uncaught <Class>: <msg> in <file>:<line>\n
//!   Stack trace:\n<trace>\n  thrown`. Parsed here.
//! * Worker — the fiber catch site pre-fills `exception_class`/`stacktrace`
//!   structurally; used directly, no parsing.

use crate::types::{CapturedException, PhpScriptError};

/// Scan a request's error stream for the terminal failure and normalize it.
/// `None` if there is none.
///
/// Prefers the actual uncaught exception (worker: `exception_class` set;
/// traditional: an `Uncaught …` fatal) over a plain error raised later by a
/// shutdown function — those run *after* the terminating error is recorded and
/// would otherwise shadow the exception that really failed the request. Falls
/// back to the last `error`-level entry when no uncaught exception is present.
pub fn extract_unhandled_exception(errors: &[PhpScriptError]) -> Option<CapturedException> {
    let err = errors
        .iter()
        .rev()
        .find(|e| {
            e.level == "error"
                && (e.exception_class.is_some() || e.message.starts_with("Uncaught "))
        })
        .or_else(|| errors.iter().rev().find(|e| e.level == "error"))?;

    // `oxphp_error_cb` substitutes the literal "unknown" for a NULL zend
    // filename; treat it as absent so `exception.file` is omitted (like
    // `line == 0`) rather than exported with a placeholder value.
    let file = (!err.file.is_empty() && err.file != "unknown").then(|| err.file.clone());
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

    // Stack trace = everything after the FIRST header line. For a chained
    // exception this keeps the whole "…\n\nNext <thrown> …" chain — useful for
    // debugging.
    let stacktrace = body
        .split_once("\nStack trace:\n")
        .map(|(_, t)| t.trim_end().to_string());

    // The exception that actually escaped is the OUTERMOST. PHP's
    // `Exception::__toString` (Zend/zend_exceptions.c) renders a chain
    // root-cause-first and appends the thrown exception after the final
    // "\n\nNext "; the `Uncaught` fatal's file:line is the thrown exception's
    // origin (`file`/`line` here). Take that last chain segment's header so
    // type/message describe the thrown exception — not the root cause — since
    // both must agree for error-inbox bucketing, and the traditional path must
    // match the worker path (which reads the class from `EG(exception)->ce`).
    let outer = body.rsplit("\n\nNext ").next().unwrap_or(body);
    let outer_header = outer
        .split_once("\nStack trace:\n")
        .map(|(h, _)| h)
        .unwrap_or(outer);

    // Strip the " in <file>:<line>" tail using the known origin, isolating
    // "<Class>[: <message>]". Robust even if the message itself contains " in ".
    let header = if !file.is_empty() && line != 0 {
        let tail = format!(" in {file}:{line}");
        outer_header.strip_suffix(&tail).unwrap_or(outer_header)
    } else {
        outer_header
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
    fn chained_takes_thrown_not_root_cause() {
        // Real PHP shape for `throw new ApiException('api failed', previous:
        // new PDOException('db down'))`: __toString renders the root cause
        // (PDOException) first and appends the thrown ApiException after the
        // final "\n\nNext "; the Uncaught fatal's file:line is the thrown one's.
        let msg = "Uncaught PDOException: db down in /db.php:10\nStack trace:\n#0 /db.php(5): connect()\n#1 {main}\n\nNext ApiException: api failed in /api.php:20\nStack trace:\n#0 /api.php(15): handle()\n#1 {main}\n  thrown";
        let c =
            extract_unhandled_exception(&[err("error", "E_ERROR", msg, "/api.php", 20)]).unwrap();
        // Bucket on the exception that actually escaped, not its root cause.
        assert_eq!(c.exception_type, "ApiException");
        assert_eq!(c.message.as_deref(), Some("api failed"));
        // No " in <file>:<line>" glued into the message.
        assert!(!c.message.as_deref().unwrap().contains(" in "));
        assert_eq!(c.file.as_deref(), Some("/api.php"));
        assert_eq!(c.line, Some(20));
        // The full chain (root cause first) survives in the stacktrace.
        let trace = c.stacktrace.as_deref().unwrap();
        assert!(trace.starts_with("#0 /db.php(5): connect()"));
        assert!(trace.contains("Next ApiException: api failed"));
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
        // A genuine classless fatal (no Throwable): E_USER_ERROR from
        // trigger_error, OOM, or a timeout. The message has no "Uncaught "
        // prefix and there is no stack trace, so the synthetic type is the
        // error constant. (An undefined-function call is NOT this shape on
        // PHP 8 — it throws a Throwable `Error` with a full trace.)
        let c = extract_unhandled_exception(&[err(
            "error",
            "E_USER_ERROR",
            "fatal path: kaboom",
            "/x.php",
            7,
        )])
        .unwrap();
        assert_eq!(c.exception_type, "E_USER_ERROR");
        assert_eq!(c.message.as_deref(), Some("fatal path: kaboom"));
        assert_eq!(c.stacktrace, None);
        assert_eq!(c.file.as_deref(), Some("/x.php"));
        assert_eq!(c.line, Some(7));
    }

    #[test]
    fn shutdown_error_does_not_shadow_uncaught() {
        // The uncaught exception is recorded first; a shutdown function then
        // raises its own fatal. The span must report the exception that killed
        // the request, not the later shutdown-time error.
        let errs = vec![
            err(
                "error",
                "E_ERROR",
                "Uncaught RuntimeException: real killer in /app.php:9\nStack trace:\n#0 {main}\n  thrown",
                "/app.php",
                9,
            ),
            err("error", "E_USER_ERROR", "shutdown logger blew up", "/shutdown.php", 3),
        ];
        let c = extract_unhandled_exception(&errs).unwrap();
        assert_eq!(c.exception_type, "RuntimeException");
        assert_eq!(c.message.as_deref(), Some("real killer"));
    }

    #[test]
    fn unknown_file_is_omitted() {
        // NULL zend filename arrives as the literal "unknown"; omit the
        // attribute rather than exporting the placeholder.
        let c =
            extract_unhandled_exception(&[err("error", "E_ERROR", "boom", "unknown", 0)]).unwrap();
        assert_eq!(c.file, None);
        assert_eq!(c.line, None);
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
