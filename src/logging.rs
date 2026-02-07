use tracing_subscriber::fmt::format::FmtSpan;
use tracing_subscriber::{fmt, EnvFilter};

/// Initialize JSON structured logging with the given default level.
/// Respects `RUST_LOG` env var if set, otherwise uses `log_level`.
pub fn init(log_level: &str) -> Result<(), crate::types::BoxError> {
    let filter = EnvFilter::try_from_default_env().or_else(|_| EnvFilter::try_new(log_level))?;

    fmt()
        .json()
        .with_env_filter(filter)
        .with_span_events(FmtSpan::NONE)
        .with_target(false)
        .with_current_span(false)
        .init();

    Ok(())
}
