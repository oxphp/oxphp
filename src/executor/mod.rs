#[cfg(feature = "php")]
pub mod sapi;
pub mod stub;

use crate::types::{ScriptRequest, ScriptResponse};

pub trait ScriptExecutor: Send + Sync {
    fn execute(&self, request: ScriptRequest) -> tokio::sync::oneshot::Receiver<ScriptResponse>;

    fn shutdown(&self);
}

/// Create executor based on `EXECUTOR` env var.
/// Returns `SapiExecutor` when compiled with `php` feature, otherwise `StubExecutor`.
pub fn create_executor() -> Box<dyn ScriptExecutor> {
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
                tracing::info!("Creating SapiExecutor (PHP mode)");
                Box::new(sapi::SapiExecutor::new())
            }
            #[cfg(not(feature = "php"))]
            {
                tracing::warn!("PHP feature not enabled, falling back to StubExecutor");
                Box::new(stub::StubExecutor::new())
            }
        }
    }
}
