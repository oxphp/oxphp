//! OxPHP\Shared — process-wide shared state layer.
//!
//! Ships Counter, Flag, Once, Mutex, Channel, Map, Pool + registry +
//! `/__ox_shared/*` API.

pub mod config;
pub mod cycle;
pub mod deadlock;
pub mod error;
pub mod eviction;
pub mod exceptions;
pub mod handle;
pub mod observability;
pub mod pool_spike;
pub mod reentrancy;
pub mod registry;
pub mod results;
pub mod types;
pub mod value;
pub mod worker_liveness;

use crate::plugin::{Plugin, PluginContext, PluginDeps, PluginError, PluginHealth};

pub struct SharedPlugin {
    /// Stored at init() time so on_ready() can start the detector inside
    /// the Tokio runtime (tokio::spawn requires an active runtime).
    lock_diagnostics: config::LockDiagnosticsLevel,
    lock_poll_interval_ms: u64,
}

impl Default for SharedPlugin {
    fn default() -> Self {
        Self {
            lock_diagnostics: config::LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
        }
    }
}

impl Plugin for SharedPlugin {
    fn name(&self) -> &'static str {
        "ox_shared"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn dependencies(&self) -> PluginDeps {
        PluginDeps::default()
    }

    fn init(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        // Register all exception classes (even those thrown by later phases,
        // so user `catch` blocks compile against the full hierarchy from v1).
        exceptions::register_all(ctx)?;

        // Channel\RecvResult / SendResult / RecvStatus / SendStatus.
        // Registered before any Shared type whose methods reference these
        // FQNs in their return-type metadata.
        results::register_all(ctx)?;

        // Register the Shared\Ordering enum first — Shared\Atomic methods
        // accept it as a parameter, so its FQN must resolve at class
        // registration time.
        {
            use crate::plugin::types::{PhpType, PhpValue};
            ctx.register_enum("OxPHP\\Shared\\Ordering")
                .backed_by(PhpType::Int)
                .case_value("Relaxed", PhpValue::Int(0))
                .case_value("Acquire", PhpValue::Int(1))
                .case_value("Release", PhpValue::Int(2))
                .case_value("AcqRel", PhpValue::Int(3))
                .case_value("SeqCst", PhpValue::Int(4))
                .build()?;
        }

        // Register the atomic type classes.
        types::counter::register_class(ctx)?;
        types::atomic::register_class(ctx)?;
        types::flag::register_class(ctx)?;
        types::once::register_enums(ctx)?;
        types::once::register_class(ctx)?;
        types::mutex::register_class(ctx)?;
        types::channel::register_class(ctx)?;
        types::map::register_class(ctx)?;
        // Shared\Pool + Shared\Pool\Handle.
        types::pool::register_classes(ctx)?;
        // Shared\Registry — name-keyed get-or-create facade.
        types::registry::register_class(ctx)?;

        // Cross-thread fcc invocation probe (temporary spike).
        pool_spike::register_functions(ctx)?;

        let cfg = config::SharedConfig::from_ctx(ctx)?;

        // Initialise the process-global registry with config.
        registry::init_registry(cfg.clone());

        // Store deadlock-detector config for on_ready() — tokio::spawn
        // requires an active Tokio runtime, which is not available at
        // init()/MINIT time.
        self.lock_diagnostics = cfg.lock_diagnostics;
        self.lock_poll_interval_ms = cfg.lock_poll_interval_ms;

        ctx.expose_config("enabled", cfg.enabled);
        ctx.expose_config("max_entries", cfg.max_entries as u64);
        ctx.expose_config("max_bytes", cfg.max_bytes);

        // Observability: internal routes + Prometheus metrics collector.
        if cfg.introspection_enabled {
            observability::register_routes(ctx)?;
        }
        if cfg.metrics_enabled {
            ctx.register_metrics(observability::SharedMetricsCollector);
        }
        if cfg.introspection_enabled || cfg.metrics_enabled {
            tracing::warn!(
                plugin = "ox_shared",
                "Deprecated Shared\\* observability names emitted alongside the new \
                 ones: Prometheus `oxphp_shared_channel_pending` (use `_count`), \
                 `oxphp_shared_pool_size` (use `_count`); JSON keys \
                 `Channel.pending` and `Pool.size` (use `.count`). The deprecated \
                 aliases will be removed in a future release — update dashboards \
                 and alerts before upgrading."
            );
        }

        tracing::info!(
            plugin = "ox_shared",
            max_entries = cfg.max_entries,
            max_bytes = cfg.max_bytes,
            "Shared plugin initialised"
        );

        Ok(())
    }

    fn on_ready(&self) {
        // Tokio runtime is now running — safe to spawn the background
        // deadlock-detector task.
        deadlock::start_detector(
            self.lock_diagnostics,
            std::time::Duration::from_millis(self.lock_poll_interval_ms),
        );
        // Shared\Pool idle-timeout eviction scheduler.
        // Idempotent — a second call is a no-op.
        eviction::start_scheduler(eviction::DEFAULT_SCAN_INTERVAL);
    }

    fn shutdown(&mut self) {
        // Drain: wake blocked ops.
        if let Some(reg) = registry::REGISTRY.get() {
            reg.drain();
        }
        tracing::info!(plugin = "ox_shared", "Shared plugin shutdown");
    }

    fn health(&self) -> PluginHealth {
        PluginHealth::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_and_version() {
        let p = SharedPlugin::default();
        assert_eq!(p.name(), "ox_shared");
        assert_eq!(p.version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn default_impl() {
        let p = SharedPlugin::default();
        assert_eq!(p.name(), "ox_shared");
    }
}
