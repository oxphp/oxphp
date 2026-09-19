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
    /// `SHARED_ENABLED`, read in init() so on_ready() can see the same
    /// decision. False until init() runs.
    enabled: bool,
    /// Stored at init() time so on_ready() can start the detector inside
    /// the Tokio runtime (tokio::spawn requires an active runtime).
    lock_diagnostics: config::LockDiagnosticsLevel,
    lock_poll_interval_ms: u64,
}

impl Default for SharedPlugin {
    fn default() -> Self {
        Self {
            enabled: false,
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
        // Config first: `SHARED_ENABLED` decides whether anything below runs,
        // and nothing here registers — `from_ctx` only reads env and plugin
        // config, so reading it before the class builders costs nothing.
        let cfg = config::SharedConfig::from_ctx(ctx)?;
        self.enabled = cfg.enabled;

        // The master switch, and it switches the whole subsystem: no classes,
        // no functions, no registry, no routes, no metrics, no background
        // tasks. PHP code that constructs an `OxPHP\Shared\*` then meets an
        // undefined class and `new` throws `Error: Class "…" not found`, which
        // is what "off" means here — from PHP's side the same surface a build
        // without the `plugin-shared` feature offers. The one name that
        // survives either way is the `OxPHP\Shared\Shareable` interface: the
        // C extension registers it in MINIT (`ext/oxphp_sapi.c`), not this
        // plugin, so `interface_exists` on it stays true with nothing to
        // implement it.
        //
        // Leaving `REGISTRY` unset is safe because every reader outside the
        // classes treats an empty one as nothing to do: `eviction::run_scan`
        // and `eviction::drain_stale_for_current_thread` return early,
        // `value::resolve_transit_keepalive` does the same, and
        // `worker_liveness::unregister_worker` reaches it only through
        // `types::pool::reclaim_all_pools_for_dead_worker`, which is guarded
        // the same way. `eviction::register`/`take_evict_request` and
        // `worker_liveness`'s own live-worker set never consult it at all. The
        // one accessor that would panic on an unset registry is reachable only
        // from the method handlers of the classes this branch does not
        // register.
        if !cfg.enabled {
            tracing::info!(
                plugin = "ox_shared",
                "Shared plugin disabled (SHARED_ENABLED != true)"
            );
            ctx.expose_config("enabled", false);
            return Ok(());
        }

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
        // Same switch as init(): a disabled plugin owns no registry for these
        // two to scan, so neither task has anything to do.
        if !self.enabled {
            return;
        }
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
    use crate::config::test_env::with_env;
    use crate::events::EventDispatcher;
    use crate::plugin::builders::definitions::{
        PhpAttributeDef, PhpClassDef, PhpEnumDef, PhpFunctionDef, PhpInterfaceDef,
    };
    use crate::plugin::php::PluginNativeFunctionDef;
    use crate::plugin::{PluginDecoratorDef, PluginInternalHandler, PluginMetricsCollector};
    use std::collections::HashMap;

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

    /// Every key `SharedConfig::from_ctx` reads. A test that runs the real
    /// `init()` has to neutralise all of them, not just the one it is about:
    /// the enabled path ends in `registry::init_registry`, and `REGISTRY` is a
    /// binary-wide `OnceLock` whose first writer wins. An ambient `SHARED_*`
    /// in the shell that started `cargo test` would therefore not merely steer
    /// this test — it would become the config every other ox_shared test sees,
    /// and those tests would fail somewhere else entirely. `SHARED_MAX_BYTES=0`
    /// reproduces it: sixty-one failures across map, pool, channel and
    /// registry, all pointing at code that is fine.
    const CONFIG_KEYS: [&str; 17] = [
        "ENABLED",
        "MAX_ENTRIES",
        "MAX_BYTES",
        "SOFT_LIMIT_RATIO",
        "METRICS_ENABLED",
        "INTROSPECTION_ENABLED",
        "INTROSPECTION_PREVIEW_ENABLED",
        "CYCLE_DETECT_DEPTH",
        "CYCLE_DETECT_EDGES",
        "MAX_VALUE_SIZE",
        "MAX_CHANNEL_BYTES",
        "POISON_STRICT",
        "LOCK_DIAGNOSTICS",
        "LOCK_POLL_INTERVAL_MS",
        "PREVIEW_STRING_LIMIT",
        "PREVIEW_ARRAY_LIMIT",
        "SHUTDOWN_TIMEOUT_SECONDS",
    ];

    /// Run `f` with every [`CONFIG_KEYS`] entry cleared in all three spellings
    /// `shared_env` tries — `SHARED_<K>`, `OX_SHARED_<K>` and the bare `<K>` —
    /// then with `overrides` applied on top. Clearing only `SHARED_<K>` would
    /// leave the other two tiers live, and the bare tier is a name as generic
    /// as `ENABLED`.
    fn with_clean_shared_env<R>(overrides: &[(&str, &str)], f: impl FnOnce() -> R) -> R {
        let mut owned: Vec<(String, Option<String>)> = Vec::new();
        for key in CONFIG_KEYS {
            owned.push((format!("SHARED_{key}"), None));
            owned.push((format!("OX_SHARED_{key}"), None));
            owned.push((key.to_string(), None));
        }
        for (key, val) in overrides {
            owned.push(((*key).to_string(), Some((*val).to_string())));
        }
        let vars: Vec<(&str, Option<&str>)> = owned
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_deref()))
            .collect();
        with_env(&vars, f)
    }

    /// Everything `SharedPlugin::init` is able to register, captured so a test
    /// can ask what the plugin put where. Mirrors the slots `PluginManager`
    /// owns in production.
    struct Registrations {
        config_values: HashMap<String, serde_json::Value>,
        metrics_collectors: usize,
        internal_routes: Vec<String>,
        native_php_functions: Vec<String>,
        php_classes: Vec<String>,
        php_enums: Vec<String>,
        php_functions: Vec<String>,
        /// Slots ox_shared does not use today, counted anyway so a later
        /// registration landing above the gate is caught. `config_values` is
        /// not among them — it is captured in its own field and the disabled
        /// test asserts its whole contents, not just the one key. Two things
        /// do stay outside every field here and outside the test's reach:
        /// event handlers, because `EventDispatcher` exposes no count, and
        /// `init_registry`, because `REGISTRY` is a binary-wide `OnceLock`
        /// other tests also set. A change that moved only those above the
        /// gate would pass.
        other_slots: usize,
    }

    /// Run `init()` against a throwaway context and report what it registered.
    fn init_shared_plugin(plugin: &mut SharedPlugin) -> Registrations {
        let mut dispatcher = EventDispatcher::new();
        let mut services: HashMap<String, Box<dyn std::any::Any + Send + Sync>> = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics_collectors: Vec<Box<dyn PluginMetricsCollector>> = Vec::new();
        let mut internal_routes: HashMap<String, Box<dyn PluginInternalHandler>> = HashMap::new();
        let mut internal_route_prefixes: Vec<(String, Box<dyn PluginInternalHandler>)> = Vec::new();
        let mut native_php_functions: Vec<PluginNativeFunctionDef> = Vec::new();
        let mut decorators: Vec<PluginDecoratorDef> = Vec::new();
        let mut php_classes: Vec<PhpClassDef> = Vec::new();
        let mut php_interfaces: Vec<PhpInterfaceDef> = Vec::new();
        let mut php_enums: Vec<PhpEnumDef> = Vec::new();
        let mut php_attributes: Vec<PhpAttributeDef> = Vec::new();
        let mut php_functions: Vec<PhpFunctionDef> = Vec::new();
        let mut core_flags = HashMap::new();

        let mut ctx = PluginContext::new(
            "ox_shared".into(),
            "__oxp_ox_shared_".into(),
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics_collectors,
            &mut internal_routes,
            &mut internal_route_prefixes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
            &mut core_flags,
        );
        plugin.init(&mut ctx).expect("init must succeed");
        drop(ctx);

        Registrations {
            config_values,
            metrics_collectors: metrics_collectors.len(),
            internal_routes: internal_routes.keys().cloned().collect(),
            native_php_functions: native_php_functions.into_iter().map(|f| f.name).collect(),
            php_classes: php_classes.into_iter().map(|c| c.fqn).collect(),
            php_enums: php_enums.into_iter().map(|e| e.fqn).collect(),
            php_functions: php_functions.into_iter().map(|f| f.fqn).collect(),
            other_slots: php_interfaces.len()
                + php_attributes.len()
                + decorators.len()
                + internal_route_prefixes.len()
                + services.len()
                + core_flags.len(),
        }
    }

    #[test]
    fn disabled_registers_no_php_surface() {
        let mut plugin = SharedPlugin::default();
        let reg = with_clean_shared_env(&[("SHARED_ENABLED", "false")], || {
            init_shared_plugin(&mut plugin)
        });

        assert!(
            reg.php_classes.is_empty(),
            "SHARED_ENABLED=false still registered classes: {:?}",
            reg.php_classes
        );
        assert!(
            reg.php_enums.is_empty(),
            "SHARED_ENABLED=false still registered enums: {:?}",
            reg.php_enums
        );
        assert!(
            reg.php_functions.is_empty() && reg.native_php_functions.is_empty(),
            "SHARED_ENABLED=false still registered functions: {:?} / {:?}",
            reg.php_functions,
            reg.native_php_functions
        );
        assert!(
            reg.internal_routes.is_empty(),
            "SHARED_ENABLED=false still registered routes: {:?}",
            reg.internal_routes
        );
        assert_eq!(
            reg.metrics_collectors, 0,
            "SHARED_ENABLED=false still registered a metrics collector"
        );
        assert_eq!(
            reg.other_slots, 0,
            "SHARED_ENABLED=false registered into a slot this plugin did not use before"
        );
        // The whole map, not just the one key: `expose_config` writes here,
        // so a registration that landed above the gate would otherwise pass.
        assert_eq!(
            reg.config_values,
            HashMap::from([("enabled".to_string(), serde_json::json!(false))]),
            "SHARED_ENABLED=false exposed config beyond `enabled`"
        );
        assert!(!plugin.enabled);
    }

    /// The other half: the gate must not swallow the default. Without it a
    /// gate placed one line too high would pass the disabled test while
    /// turning the subsystem off for everyone.
    #[test]
    fn enabled_registers_the_whole_surface() {
        let mut plugin = SharedPlugin::default();
        let reg = with_clean_shared_env(&[], || init_shared_plugin(&mut plugin));

        // Every name `init` registers today, not a sample: the exception
        // hierarchy, the result objects, the nine type families and their
        // nested companions. `init` reaches them through six separate calls,
        // and a gate landing between any two of them would leave a plausible
        // subset behind — which is what a sample would miss.
        for fqn in [
            "OxPHP\\Shared\\SharedException",
            "OxPHP\\Shared\\StaleHandleException",
            "OxPHP\\Shared\\TypeException",
            "OxPHP\\Shared\\CapacityException",
            "OxPHP\\Shared\\ValueTooLargeException",
            "OxPHP\\Shared\\ClosedException",
            "OxPHP\\Shared\\PoisonedException",
            "OxPHP\\Shared\\UninitializedException",
            "OxPHP\\Shared\\InvalidOrderingException",
            "OxPHP\\Shared\\CorruptedMutexException",
            "OxPHP\\Shared\\CycleException",
            "OxPHP\\Shared\\OperationTimeoutException",
            "OxPHP\\Shared\\ContentionException",
            "OxPHP\\Shared\\DeadlockException",
            "OxPHP\\Shared\\Channel\\RecvResult",
            "OxPHP\\Shared\\Channel\\SendResult",
            "OxPHP\\Shared\\Counter",
            "OxPHP\\Shared\\Atomic",
            "OxPHP\\Shared\\Flag",
            "OxPHP\\Shared\\Once",
            "OxPHP\\Shared\\Mutex",
            "OxPHP\\Shared\\Channel",
            "OxPHP\\Shared\\Map",
            "OxPHP\\Shared\\Map\\KeyCursor",
            "OxPHP\\Shared\\Pool",
            "OxPHP\\Shared\\Pool\\Handle",
            "OxPHP\\Shared\\Pool\\Stats",
            "OxPHP\\Shared\\Registry",
        ] {
            assert!(
                reg.php_classes.iter().any(|c| c == fqn),
                "default config did not register {fqn}: {:?}",
                reg.php_classes
            );
        }
        for fqn in [
            "OxPHP\\Shared\\Ordering",
            "OxPHP\\Shared\\Once\\Status",
            "OxPHP\\Shared\\Once\\FailureMode",
            "OxPHP\\Shared\\Channel\\RecvStatus",
            "OxPHP\\Shared\\Channel\\SendStatus",
        ] {
            assert!(
                reg.php_enums.iter().any(|e| e == fqn),
                "default config did not register enum {fqn}: {:?}",
                reg.php_enums
            );
        }
        for name in [
            "oxphp_pool_spike_capture",
            "oxphp_pool_spike_invoke",
            "oxphp_pool_spike_reset",
        ] {
            assert!(
                reg.php_functions.iter().any(|f| f == name),
                "default config did not register {name}(): {:?}",
                reg.php_functions
            );
        }
        assert!(reg
            .internal_routes
            .iter()
            .any(|r| r.starts_with("/__ox_shared/")));
        assert_eq!(reg.metrics_collectors, 1);
        assert_eq!(
            reg.config_values.get("enabled"),
            Some(&serde_json::json!(true))
        );
        assert!(plugin.enabled);
    }

    /// `on_ready` reaches `tokio::spawn`, which panics outside a runtime, so
    /// "did not panic" is a statement about the gate.
    ///
    /// Exactly one of the two starters carries that weight. The detector is
    /// not it: this path never assigns `self.lock_diagnostics`, which the
    /// gate returns before, so it keeps `Default`'s `Off` and
    /// `deadlock::start_detector` returns on the level check whether the gate
    /// is there or not. (`parse_lock_diag` does run — `from_ctx` is the first
    /// thing `init()` does — so its `Strict`-under-`debug_assertions` default
    /// is computed; it just stops at the local `cfg`.) The eviction
    /// scheduler is it — and only while nothing has tripped its one-shot
    /// latch, since a tripped latch also returns before the spawn. Nothing
    /// else in the crate calls `start_scheduler` and no other test calls
    /// `on_ready`, so the latch is false here; the assertion below says so out
    /// loud, and turns a future second caller into a failure here rather than
    /// a test that silently stops proving anything.
    #[test]
    fn on_ready_starts_nothing_when_disabled() {
        let mut plugin = SharedPlugin::default();
        with_clean_shared_env(&[("SHARED_ENABLED", "false")], || {
            let _ = init_shared_plugin(&mut plugin);
        });

        assert!(
            !eviction::scheduler_running(),
            "the eviction scheduler was already started, so this test can no \
             longer tell a working gate from a missing one"
        );

        plugin.on_ready();
    }
}
