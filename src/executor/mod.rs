#[cfg(feature = "php")]
pub mod inline;
#[cfg(feature = "php")]
pub mod sapi;
pub mod stub;

use std::sync::Arc;

use crate::metrics::Metrics;
use crate::types::{ScriptRequest, ScriptResponse};

/// Result of executor dispatch. Stub returns `Immediate` (no channel overhead),
/// SAPI returns `Deferred` (worker thread sends response via oneshot).
pub enum ExecuteResult {
    /// Response available immediately (no async wait needed).
    Immediate(ScriptResponse),
    /// Response will arrive via oneshot channel from a worker thread.
    Deferred(tokio::sync::oneshot::Receiver<ScriptResponse>),
}

pub trait ScriptExecutor: Send + Sync {
    fn execute(&self, request: ScriptRequest) -> ExecuteResult;

    fn shutdown(&self);

    /// Check if the executor is healthy and can accept requests.
    fn is_healthy(&self) -> bool {
        true
    }

    /// Start the scale manager if the executor supports dynamic scaling.
    /// Called from async context (Tokio runtime). Default: no-op.
    fn start_scale_manager(&self) {}
}

/// Create executor based on `EXECUTOR` env var.
/// Returns `SapiExecutor` when compiled with `php` feature, otherwise `StubExecutor`.
pub fn create_executor(metrics: Arc<Metrics>) -> Box<dyn ScriptExecutor> {
    let executor_type = std::env::var("EXECUTOR")
        .unwrap_or_else(|_| "sapi".to_string())
        .to_lowercase();

    match executor_type.as_str() {
        "stub" => {
            tracing::info!("Creating StubExecutor (benchmark mode)");
            Box::new(stub::StubExecutor::new())
        }
        _ => {
            #[cfg(feature = "php")]
            {
                let _ = metrics;
                tracing::info!("Creating InlineExecutor (PHP mode)");
                Box::new(inline::InlineExecutor::new())
            }
            #[cfg(not(feature = "php"))]
            {
                let _ = metrics;
                tracing::warn!("PHP feature not enabled, falling back to StubExecutor");
                Box::new(stub::StubExecutor::new())
            }
        }
    }
}
