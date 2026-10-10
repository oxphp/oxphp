pub mod admission;
pub mod async_fiber;
pub mod async_pool;
pub(crate) mod idle_clock;
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

/// A request that is in the queue, and the moment it stops being worth
/// starting — `None` in fail-fast mode, where there is no wait to bound.
///
/// The deadline travels with the channel because the pool cannot enforce it on
/// its own: it is read at pickup, and a request nobody picks up is never read
/// at all. The waiting side holds the only clock that keeps running in that
/// case.
pub struct Queued {
    pub rx: DeferredResponse,
    pub deadline: Option<std::time::Instant>,
    /// Whether `deadline` was stamped from the configured wait budget rather
    /// than a shortened one. Carried from arrival because the waiting side
    /// asks, when the deadline answers a request no worker took, whether the
    /// pool started anything at all while it waited — a question only a
    /// full-length window can put to the pool.
    pub wait_at_ceiling: bool,
}

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
    /// The request is in the queue. The answer usually arrives through the
    /// oneshot channel from a worker thread — but see [`Queued`]: when the
    /// deadline passes with nobody having taken the request, the waiting side
    /// answers it itself and no worker is involved.
    Deferred(Queued),
    /// The queue was full and the request is waiting for a slot. Resolves to
    /// `Ok` once admitted (equivalent to `Deferred` from there on), or to
    /// `Err` with a synthesized response once the request is refused — the
    /// `Rejected` case, reached asynchronously. Boxed because only this
    /// contended path needs a future — the admitted path stays synchronous
    /// and allocation-free.
    Admitting(
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<Queued, ScriptResponse>> + Send>>,
    ),
}

pub trait ScriptExecutor: Send + Sync {
    /// Whoever takes the request off the queue calls
    /// `request.cancel_state.mark_taken()` at that moment, before answering it
    /// and whatever it then does with it: the dispatch side measures the
    /// request's queue wait up to that stamp, and records none for a request
    /// that was never marked. A request taken after its budget ran out is
    /// answered with `refused` set, or the whole budget is recorded as a wait.
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

/// Ends the process when a panic unwinds the thread while this guard is alive.
///
/// A PHP thread holds one while a PHP request is open on it, from a successful
/// `php_request_startup()` to `php_request_shutdown()`. A panic that unwinds
/// out of that window is not recovered from. Shutting the request down would
/// call back into the Rust state the panic abandoned: RSHUTDOWN, the output and
/// header callbacks, the error callback, and any native function a shutdown
/// function or destructor calls all reach it. Releasing the thread's TSRM entry
/// with the request still open would leave behind what only that shutdown
/// tears down, the engine's execution timer among them. An async pool thread
/// holds one for its whole body, since nothing would replace it.
///
/// Only a panic in the thread's own Rust frames gets this far. One raised in a
/// callback PHP makes into Rust aborts at that callback's `extern "C"`
/// boundary first.
///
/// Dropped with no panic in flight it does nothing: a return from inside the
/// window still has to shut the request down first.
#[cfg_attr(not(feature = "php"), allow(dead_code))]
pub(crate) struct AbortOnUnwind(pub(crate) &'static str);

impl Drop for AbortOnUnwind {
    fn drop(&mut self) {
        if std::thread::panicking() {
            // Written straight to stderr: the log writer hands lines to a
            // background thread, and abort() discards what it has not written.
            let thread = std::thread::current();
            eprintln!(
                "oxphp: thread '{}' panicked {}; aborting",
                thread.name().unwrap_or("<unnamed>"),
                self.0
            );
            std::process::abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AbortOnUnwind;

    const CHILD: &str = "OXPHP_TEST_ABORT_ON_UNWIND_CHILD";

    #[test]
    fn abort_on_unwind_aborts_a_thread_that_panics_under_it() {
        if std::env::var_os(CHILD).is_some() {
            let _guard = AbortOnUnwind("under the guard");
            panic!("unwinding through the guard");
        }
        // The abort takes the whole process with it, so the panic runs in a
        // copy of this test binary. --nocapture: captured output would go down
        // with the process.
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "executor::tests::abort_on_unwind_aborts_a_thread_that_panics_under_it",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .output()
            .unwrap();
        let stderr = String::from_utf8_lossy(&out.stderr);
        use std::os::unix::process::ExitStatusExt;
        assert_eq!(out.status.signal(), Some(libc::SIGABRT), "{stderr}");
        assert!(
            stderr.contains("panicked under the guard; aborting"),
            "{stderr}"
        );
    }

    #[test]
    fn abort_on_unwind_lets_a_thread_go_once_it_is_dropped() {
        drop(AbortOnUnwind("under the guard"));
        let joined = std::thread::spawn(|| {
            drop(AbortOnUnwind("under the guard"));
            panic!("after the guard");
        })
        .join();
        assert!(joined.is_err());
    }
}
