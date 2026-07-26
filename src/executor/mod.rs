pub mod admission;
pub mod async_fiber;
pub mod async_pool;
#[cfg(feature = "php")]
pub mod sapi;
pub mod stub;

pub use crate::config::WorkerMode;

use std::sync::Arc;

use crate::config::Config;
use crate::metrics::Metrics;
use crate::types::{ScriptRequest, ScriptResponse};

/// Response channel for a request a worker has accepted.
type DeferredResponse = tokio::sync::oneshot::Receiver<ScriptResponse>;

/// Result of executor dispatch. Stub returns `Immediate` (no channel overhead),
/// SAPI returns `Deferred` (worker thread sends response via oneshot).
pub enum ExecuteResult {
    /// Response available immediately (no async wait needed).
    Immediate(ScriptResponse),
    /// The request was refused without reaching a worker — shed under overload,
    /// or answered from a dead pool. Carries the synthesized response.
    ///
    /// Distinct from `Immediate` so callers can tell a response the pool
    /// produced from one produced *instead of* the pool: per-request timings
    /// mean nothing here, and folding these into latency or queue-wait
    /// statistics reports a refusal as if it were work done.
    Rejected(ScriptResponse),
    /// Response will arrive via oneshot channel from a worker thread.
    Deferred(DeferredResponse),
    /// The queue was full and the request is waiting for a slot. Resolves to
    /// `Ok` once admitted (equivalent to `Deferred` from there on), or to
    /// `Err` with a synthesized response once the request is refused — the
    /// `Rejected` case, reached asynchronously. Boxed because only this
    /// contended path needs a future — the admitted path stays synchronous
    /// and allocation-free.
    Admitting(
        std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<DeferredResponse, ScriptResponse>> + Send>,
        >,
    ),
}

pub trait ScriptExecutor: Send + Sync {
    fn execute(&self, request: ScriptRequest) -> ExecuteResult;

    fn shutdown(&self);

    /// The drain deadline has passed: nothing that is not already running can
    /// still be served, so stop admitting and answer whatever is still waiting
    /// to be admitted.
    ///
    /// Deliberately not called at the *start* of the drain. Until the deadline
    /// the pool is fully operational and the drain window exists precisely so
    /// in-flight work finishes — refusing a request that raced the GOAWAY, and
    /// that the pool would have served in microseconds, is not a graceful
    /// stop. After the deadline the opposite holds: a request still waiting
    /// for admission is not in any worker, so the hard cancel does not reach
    /// it, and without this it would simply have its connection dropped when
    /// the runtime is torn down — no HTTP response at all.
    ///
    /// Default: no-op, for executors with no admission gate to close.
    fn close_admission(&self) {}

    /// Check if the executor is healthy and can accept requests.
    fn is_healthy(&self) -> bool {
        true
    }

    /// Start the scale manager if the executor supports dynamic scaling.
    /// Called from async context (Tokio runtime). Default: no-op.
    fn start_scale_manager(&self) {}
}

/// Create executor based on `Config::executor_type` (set from the `EXECUTOR`
/// env var, normalized to lowercase in `Config::from_env`). Returns
/// `SapiExecutor` when compiled with `php` feature, otherwise `StubExecutor`.
pub fn create_executor(config: &Config, metrics: Arc<Metrics>) -> Box<dyn ScriptExecutor> {
    match config.executor_type.as_str() {
        "stub" => {
            tracing::info!("Creating StubExecutor (benchmark mode)");
            Box::new(stub::StubExecutor::new())
        }
        _ => {
            #[cfg(feature = "php")]
            {
                tracing::info!("Creating SapiExecutor (PHP mode)");
                Box::new(sapi::SapiExecutor::new(config, metrics))
            }
            #[cfg(not(feature = "php"))]
            {
                let _ = (config, metrics);
                tracing::warn!("PHP feature not enabled, falling back to StubExecutor");
                Box::new(stub::StubExecutor::new())
            }
        }
    }
}
