use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{fmt, EnvFilter};

/// Initialize JSON structured logging with non-blocking writes.
/// Returns a `WorkerGuard` that MUST be held until shutdown — dropping it
/// flushes the buffer and stops the background writer thread.
///
/// Log level is read from `RUST_LOG` (full `EnvFilter` syntax) with fallback
/// to `LOG_LEVEL` (single level, default `info`). Called before config parsing
/// so startup errors are emitted as JSON.
pub fn init() -> Result<WorkerGuard, crate::types::BoxError> {
    let fallback = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());
    let filter = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new(&fallback))?;

    let (non_blocking, guard) = tracing_appender::non_blocking(std::io::stdout());

    fmt()
        .json()
        .with_writer(non_blocking)
        .with_env_filter(filter)
        .with_span_events(FmtSpan::NONE)
        .with_target(false)
        .with_current_span(false)
        .init();

    Ok(guard)
}
