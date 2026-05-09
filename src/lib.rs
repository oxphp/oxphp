pub mod async_types;
pub(crate) mod bridge;
pub mod cli;
pub mod config;
pub mod decorator;
pub mod events;
pub mod executor;
pub mod handlers;
pub mod metrics;
pub mod php;
pub mod plugin;
pub mod plugins;
pub mod profiling;
pub mod server;
pub mod trace_context;
pub mod types;

/// Called from the C SAPI's zend_interrupt_function override (see
/// ext/oxphp_sapi.c::oxphp_zend_interrupt_handler) right before it
/// bails through zend_error_noreturn for a cancelled request.
/// `reason` matches the oxphp_cancel_reason_t enum (1=client_abort,
/// 2=timeout, 3=shutdown). Other values are ignored. No-op if metrics
/// haven't been installed yet (e.g. during early startup).
#[no_mangle]
pub extern "C" fn oxphp_metrics_cancelled(reason: u8) {
    if let Some(m) = crate::metrics::GLOBAL_METRICS.get() {
        m.observe_cancelled(reason);
    }
}
