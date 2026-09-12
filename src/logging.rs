use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::{fmt, EnvFilter};

use oxphp::config::{parse_access_log, AccessLogLevel};
use oxphp::handlers::access_log::TARGET as ACCESS_LOG_TARGET;

/// Initialize JSON structured logging with non-blocking writes.
/// Returns a `WorkerGuard` that MUST be held until shutdown — dropping it
/// flushes the buffer and stops the background writer thread.
///
/// Log level is read from `RUST_LOG` (full `EnvFilter` syntax) with fallback
/// to `LOG_LEVEL` (documented as a single level, default `info`), and carries
/// the access log's own directive on top when `ACCESS_LOG` asks for one — see
/// `with_access_log`. Called before config parsing so startup errors are
/// emitted as JSON.
pub fn init() -> Result<WorkerGuard, crate::types::BoxError> {
    let access_log = access_log_enabled();
    let (filter, directives) = build_filter(
        std::env::var("RUST_LOG").ok(),
        std::env::var("LOG_LEVEL").ok(),
        access_log,
    )?;

    let (non_blocking, guard) = tracing_appender::non_blocking(std::io::stdout());

    fmt()
        .json()
        .with_writer(non_blocking)
        .with_env_filter(filter)
        .with_span_events(FmtSpan::NONE)
        .with_target(false)
        .with_current_span(false)
        .init();

    // The filter carries a directive for the access log's target, so an enabled
    // access log can still write nothing only where a directive names that
    // target and switches it off — which wins, and which is worth saying out
    // loud, because every other way of checking the setting reports it as on.
    // The answer comes from the call site interest the installed subscriber has
    // just computed, so it describes the stack that is actually running rather
    // than our model of it.
    if access_log && !tracing::enabled!(target: ACCESS_LOG_TARGET, tracing::Level::INFO) {
        let value = std::env::var("ACCESS_LOG").unwrap_or_default();
        if tracing::enabled!(tracing::Level::WARN) {
            tracing::warn!(
                access_log = %value,
                filter = %directives,
                "ACCESS_LOG is set but the log filter drops the access_log target — no access log entries will be written"
            );
        } else {
            // The report is a log line, so a filter quiet enough to drop the
            // access log can drop the report with it and leave exactly the
            // silence this check exists to break: a server asked for an access
            // log, writing none, saying nothing. stderr is not the subscriber's
            // to filter.
            eprintln!(
                "WARN ACCESS_LOG is set but the log filter drops the access_log target — no access log entries will be written (access_log={value}, filter={directives})"
            );
        }
    }

    Ok(guard)
}

/// True when `ACCESS_LOG` asks for an access log at all.
///
/// A value this server does not recognise counts as off, the same way
/// `Config::from_env` resolves it — which is also where it is reported.
fn access_log_enabled() -> bool {
    matches!(
        parse_access_log(std::env::var("ACCESS_LOG").ok().as_deref()),
        Some(AccessLogLevel::All | AccessLogLevel::Error)
    )
}

/// Compose the filter directives and parse them, returning the filter and the
/// string it was built from.
///
/// `RUST_LOG` carries full `EnvFilter` syntax and wins where it parses; a value
/// that does not parse falls back to `LOG_LEVEL` instead of aborting startup.
/// `LOG_LEVEL` goes to the same parser, so it is not restricted to a bare level
/// either — but it is the last stop, and a value it rejects aborts startup
/// rather than falling through to a default.
fn build_filter(
    rust_log: Option<String>,
    log_level: Option<String>,
    access_log: bool,
) -> Result<(EnvFilter, String), crate::types::BoxError> {
    let base = match rust_log {
        Some(raw) if EnvFilter::try_new(with_access_log(&raw, access_log)).is_ok() => raw,
        _ => log_level.unwrap_or_else(|| "info".to_string()),
    };

    // The access log's directive is kept only where it carries something.
    // Going first means an operator's own directive for the same target
    // replaces it — and a replaced directive has still raised the process-wide
    // maximum to INFO on its way in, because that maximum only ever rises. That
    // is the one cost `info` was chosen to keep down, and here it would buy
    // nothing: the entries the directive exists to admit are dropped anyway. So
    // it is composed, asked whether it took, and dropped again where it did
    // not. The two filters behave identically; only the maximum differs.
    let directives = with_access_log(&base, access_log);
    if access_log && !admits_access_log(&directives) {
        let filter = EnvFilter::try_new(&base)?;
        return Ok((filter, base));
    }

    let filter = EnvFilter::try_new(&directives)?;
    Ok((filter, directives))
}

/// Whether a filter built from `directives` lets an access log entry through.
///
/// Asked of a throwaway subscriber rather than of the string, because which of
/// two directives naming the same target wins, and whether a shorter name
/// counts as naming it at all, are `EnvFilter`'s rules to apply — deciding it
/// here would be a second copy of them, kept in step with the first by hand.
fn admits_access_log(directives: &str) -> bool {
    let Ok(filter) = EnvFilter::try_new(directives) else {
        return false;
    };
    let probe = tracing_subscriber::registry().with(filter);
    tracing::subscriber::with_default(
        probe,
        || tracing::enabled!(target: ACCESS_LOG_TARGET, tracing::Level::INFO),
    )
}

/// Prepend the access log's own directive to `base` when the access log is on.
///
/// The access log is an audit trail rather than server diagnostics, and an
/// operator quietening the latter does not normally mean to switch off the
/// former — so `ACCESS_LOG` decides whether those lines are written and
/// `LOG_LEVEL` no longer gets a second vote. Mechanically, a directive naming a
/// target is more specific than a bare level, and `EnvFilter` answers from the
/// most specific directive that matches, so `access_log=info` before a `warn`
/// admits the entries the bare `warn` would drop.
///
/// It goes first because directives for the same target replace one another as
/// they are added: an operator who spells `access_log=…` out in `RUST_LOG`
/// still has the last word, including to turn it off. Losing that way leaves
/// the directive carrying nothing, so `build_filter` composes this string,
/// checks whether the directive took, and falls back to `base` where it did
/// not — see the comment there for what a kept-but-beaten directive costs.
///
/// `info` rather than `trace` puts the process-wide maximum no higher than the
/// default configuration already puts it, so the cheap check that elides
/// `trace!`/`debug!` call sites everywhere else is not given up for this. Under
/// a quieter `LOG_LEVEL` that maximum does rise to INFO — which is the price of
/// admitting these entries at all — and a DEBUG or TRACE event on this target
/// would then be filtered even under a verbose `LOG_LEVEL`. The handler emits
/// only `info!`, so there is none to lose.
fn with_access_log(base: &str, access_log: bool) -> String {
    if access_log {
        format!("{ACCESS_LOG_TARGET}=info,{base}")
    } else {
        base.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::sync::{Arc, Mutex};
    use tracing::level_filters::LevelFilter;
    use tracing_subscriber::fmt::MakeWriter;

    /// Collects everything the subscriber writes, so a test can ask what came
    /// out rather than what the filter was built from.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Captured {
        type Writer = Captured;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    /// Run `emit` under a subscriber built from these three inputs and return
    /// what it wrote. Each caller keeps its own `tracing::` call site, so no two
    /// tests share a cached callsite interest.
    fn under(
        rust_log: Option<&str>,
        log_level: Option<&str>,
        access_log: bool,
        emit: impl FnOnce(),
    ) -> String {
        let (filter, _) = build_filter(
            rust_log.map(str::to_string),
            log_level.map(str::to_string),
            access_log,
        )
        .expect("filter");
        let captured = Captured::default();
        let subscriber = fmt()
            .json()
            .with_writer(captured.clone())
            .with_env_filter(filter)
            .finish();
        tracing::subscriber::with_default(subscriber, emit);
        let bytes = captured.0.lock().unwrap().clone();
        String::from_utf8(bytes).expect("utf-8 log")
    }

    #[test]
    fn access_log_survives_a_quiet_log_level() {
        let out = under(None, Some("warn"), true, || {
            tracing::info!(target: ACCESS_LOG_TARGET, "request completed");
        });
        assert!(
            out.contains("request completed"),
            "LOG_LEVEL=warn dropped the access log entry: {out:?}"
        );
    }

    #[test]
    fn a_quiet_log_level_still_silences_everything_else() {
        let out = under(None, Some("warn"), true, || {
            tracing::info!("server chatter");
        });
        assert!(
            out.is_empty(),
            "the access log directive turned the whole log up: {out:?}"
        );
    }

    #[test]
    fn warnings_on_the_access_log_target_still_pass() {
        let out = under(None, Some("error"), true, || {
            tracing::warn!(target: ACCESS_LOG_TARGET, "louder than info");
        });
        assert!(
            out.contains("louder than info"),
            "the access log directive swallowed a WARN on its own target: {out:?}"
        );
    }

    #[test]
    fn an_explicit_rust_log_directive_wins() {
        let out = under(Some("warn,access_log=off"), None, true, || {
            tracing::info!(target: ACCESS_LOG_TARGET, "request completed");
        });
        assert!(
            out.is_empty(),
            "an operator switching the target off in RUST_LOG was overridden: {out:?}"
        );
    }

    #[test]
    fn an_access_log_that_is_off_leaves_the_filter_alone() {
        let out = under(None, Some("warn"), false, || {
            tracing::info!(target: ACCESS_LOG_TARGET, "request completed");
        });
        assert!(
            out.is_empty(),
            "the directive was added with ACCESS_LOG unset: {out:?}"
        );
    }

    #[test]
    fn an_unparsable_rust_log_falls_back_to_log_level() {
        let out = under(Some("this is not a filter"), Some("warn"), true, || {
            tracing::info!(target: ACCESS_LOG_TARGET, "request completed");
        });
        assert!(
            out.contains("request completed"),
            "the LOG_LEVEL fallback lost the access log: {out:?}"
        );

        let out = under(Some("this is not a filter"), Some("warn"), true, || {
            tracing::info!("server chatter");
        });
        assert!(
            out.is_empty(),
            "the LOG_LEVEL fallback did not apply: {out:?}"
        );
    }

    /// The startup report is a log line too, so a filter quiet enough to drop
    /// the access log can drop the report with it — that is the case `init()`
    /// answers on stderr instead. `init()` itself installs a global subscriber
    /// and cannot be called from a test, so the branch's two inputs are pinned
    /// here: nothing else in CI reaches them.
    #[test]
    fn a_filter_below_warn_would_swallow_the_report_too() {
        let out = under(Some("error,access_log=off"), None, true, || {
            tracing::info!(target: ACCESS_LOG_TARGET, "request completed");
            tracing::warn!("ACCESS_LOG is set but the log filter drops the access_log target");
        });
        assert!(
            out.is_empty(),
            "the report would have reached the log after all: {out:?}"
        );

        // And the documented spelling leaves it a way through, which is why the
        // stderr line is a fallback rather than the only route.
        let out = under(Some("warn,access_log=off"), None, true, || {
            tracing::warn!("ACCESS_LOG is set but the log filter drops the access_log target");
        });
        assert!(
            out.contains("drops the access_log target"),
            "a filter that admits WARN dropped the report: {out:?}"
        );
    }

    /// The directive frees one target; it must not raise the level the process
    /// publishes, because that is what lets `trace!`/`debug!` call sites
    /// everywhere else be skipped without consulting a filter. `trace` in place
    /// of `info` passes every other test in this module and fails this one.
    #[test]
    fn the_directive_does_not_raise_the_process_wide_maximum() {
        let (quiet, _) = build_filter(None, Some("warn".to_string()), true).expect("filter");
        assert_eq!(
            quiet.max_level_hint(),
            Some(LevelFilter::INFO),
            "the access log directive published a maximum above INFO"
        );

        let (default, _) = build_filter(None, None, false).expect("filter");
        assert_eq!(
            default.max_level_hint(),
            Some(LevelFilter::INFO),
            "the default stopped publishing INFO"
        );
    }

    #[test]
    fn a_directive_the_operator_overrules_is_dropped_again() {
        // Both filters behave the same — the operator's `access_log=off`
        // replaces ours either way — so the difference this pins is the
        // process-wide maximum. Keeping a beaten directive holds it at INFO,
        // which is what choosing `info` over `trace` was meant to avoid, in the
        // one case where it admits nothing in return.
        for (rust_log, level) in [
            ("warn,access_log=off", LevelFilter::WARN),
            ("error,access_log=off", LevelFilter::ERROR),
        ] {
            let (filter, directives) =
                build_filter(Some(rust_log.to_string()), None, true).expect("filter");
            assert_eq!(
                directives, rust_log,
                "a directive the operator had already overruled was kept"
            );
            assert_eq!(
                filter.max_level_hint(),
                Some(level),
                "an overruled directive left the process-wide maximum raised"
            );
        }
    }

    #[test]
    fn an_overruled_directive_changes_nothing_about_what_is_written() {
        // The point of dropping it is that nothing else moves: the entries were
        // already gone, and the rest of the log is still whatever the operator
        // asked for.
        let out = under(Some("warn,access_log=off"), None, true, || {
            tracing::info!(target: ACCESS_LOG_TARGET, "request completed");
            tracing::warn!("something the operator asked to hear about");
        });
        assert!(
            !out.contains("request completed"),
            "the entries came back: {out:?}"
        );
        assert!(
            out.contains("something the operator asked to hear about"),
            "dropping the directive silenced a warning the filter admits: {out:?}"
        );
    }

    #[test]
    fn the_default_level_is_unchanged() {
        let out = under(None, None, false, || {
            tracing::info!("server chatter");
        });
        assert!(
            out.contains("server chatter"),
            "the default level stopped passing INFO: {out:?}"
        );

        let out = under(None, None, false, || {
            tracing::debug!("server detail");
        });
        assert!(
            out.is_empty(),
            "the default level started passing DEBUG: {out:?}"
        );
    }
}
