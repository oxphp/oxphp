//! The `ox_profiler` plugin — per-request profiling activation.
//!
//! Reads `PROFILER_*` env vars at startup, inspects each incoming request for
//! a cookie / header / query-string trigger (or random sampling), and — when
//! activation is decided — sets `ProfilingMode::ProfileAll` so the PHP worker
//! runs its request under full profiling.

pub mod config;
pub mod php_sdk;
pub mod routes;
pub mod storage;
pub mod trigger;

pub use self::trigger::{ActivationDecision, ActivationSource};

use crate::events::Priority;
use crate::plugin::handler::{
    PluginCompleteHandler, PluginCompleteView, PluginRequestActions, PluginRequestHandler,
    PluginRequestView,
};
use crate::plugin::{Plugin, PluginContext, PluginDeps, PluginError, PluginHealth};
use crate::profiling::ProfilingMode;

use self::config::ProfilerConfig;
use self::trigger::should_profile;

/// Per-request profiling activation plugin.
///
/// Feature-gated behind `plugin-profiler`. Standalone — does not depend on
/// `ox_apm` or `ox_otel`.
pub struct ProfilerPlugin {
    enabled: bool,
    config: ProfilerConfig,
}

impl Default for ProfilerPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl ProfilerPlugin {
    pub fn new() -> Self {
        Self {
            enabled: false,
            config: ProfilerConfig::default(),
        }
    }
}

/// Runs on the Tokio thread. Checks the trigger; if activated, writes the
/// decision into `PluginRequestActions` so the worker will pick it up at
/// RINIT.
struct ProfilerRequestHandler {
    config: ProfilerConfig,
}

impl PluginRequestHandler for ProfilerRequestHandler {
    fn handle(&self, view: &PluginRequestView, actions: &mut PluginRequestActions) {
        // `rand::rng()` is a TLS-backed handle — zero lock
        // contention across Tokio workers and no allocation. Each
        // worker gets its own auto-seeded generator on first use.
        let mut rng = rand::rng();
        if let Some(decision) = should_profile(view, &self.config, &mut rng) {
            tracing::debug!(
                plugin = "profiler",
                source = ?decision.source,
                run_id = %decision.run_id,
                request_id = view.request_id,
                "Profiling activated"
            );
            actions.set_profiling_decision(decision.mode, decision.run_id);
        }
    }

    fn priority(&self) -> Priority {
        // Runs after trace_context (-95), before otel (-80) — same band as the
        // APM request handler so trace metadata is already set by the time we
        // mint a profiling run_id.
        -85
    }
}

/// Runs on the Tokio thread. Builds a `RunMeta` from the view +
/// tree, populates the in-memory cache eagerly, and tokio::spawns
/// the disk write + HTTP push tasks so they run in parallel
/// without blocking the request flow.
///
/// Falls back to the previous log-and-drop behaviour when the
/// profiler is not enabled or the tree's mode != ProfileAll.
/// Admission bound for each backend's spawn fan-out. Picked so a
/// single backend stall (say, xhgui down for 30 s while profiles
/// arrive at ~10/s) can't accumulate arbitrarily many in-flight
/// tasks each carrying an Arc<SpanTree> + rendered bytes. 64 is
/// generous enough that healthy backends never see backpressure
/// and small enough that a stall caps memory at tens of MB of
/// profile payloads rather than hundreds.
const DISK_SPAWN_LIMIT: usize = 64;
const HTTP_SPAWN_LIMIT: usize = 64;
/// Log rate-limit for saturation warnings: emit one warn! per N
/// drops so a backend outage doesn't spam the log at ingress rate.
const SATURATED_WARN_EVERY: u64 = 100;

struct ProfilerCompleteHandler {
    storage: std::sync::Arc<storage::Storage>,
    output_formats: Vec<storage::OutputFormat>,
    xhgui_envelope: bool,
    disk_spawn_sem: std::sync::Arc<tokio::sync::Semaphore>,
    http_spawn_sem: std::sync::Arc<tokio::sync::Semaphore>,
}

impl PluginCompleteHandler for ProfilerCompleteHandler {
    fn handle(&self, view: &PluginCompleteView) {
        let tree = match view.profile_tree {
            Some(t) => t,
            None => return,
        };
        if tree.mode != ProfilingMode::ProfileAll {
            return;
        }

        // Use request_id as the run_id (already path-safe hex).
        // The trigger's run_id is not yet plumbed back to
        // PluginCompleteView; a future change may extend the view
        // to carry it.
        let run_id = view.request_id.to_string();

        let meta = build_run_meta(view, tree, &run_id);

        // Counter bumps before the fan-out so metrics reflect every
        // dispatched run regardless of disk/http success.
        self.storage.metrics.inc_runs(meta.source);
        self.storage.metrics.add_spans(tree.finished.len() as u64);
        if meta.truncated {
            self.storage.metrics.inc_truncated();
        }

        // Cache eagerly so internal routes can read the freshest
        // tree without waiting for the async disk write.
        self.storage
            .cache
            .put(run_id.clone(), std::sync::Arc::clone(tree));

        if let Some(disk) = self.storage.disk.clone() {
            // Admission gate: try_acquire_owned returns immediately.
            // On success the permit rides into the task; its Drop at
            // task end releases the slot. On failure the backend is
            // saturated and we shed load by dropping the run.
            match std::sync::Arc::clone(&self.disk_spawn_sem).try_acquire_owned() {
                Ok(permit) => {
                    let m = meta.clone();
                    let t = std::sync::Arc::clone(tree);
                    let formats = self.output_formats.clone();
                    let envelope = self.xhgui_envelope;
                    tokio::spawn(async move {
                        // `_permit` is intentionally unused — its Drop
                        // on task end releases the admission slot.
                        let _permit = permit;
                        disk.write_run(&m, &t, &formats, envelope).await;
                    });
                }
                Err(_) => {
                    let count = self.storage.metrics.inc_disk_saturated_drop();
                    if count.is_multiple_of(SATURATED_WARN_EVERY) {
                        tracing::warn!(
                            plugin = "profiler",
                            run_id = %meta.run_id,
                            drops = count,
                            limit = DISK_SPAWN_LIMIT,
                            "disk spawn admission saturated; dropping profile run"
                        );
                    }
                }
            }
        }

        if let Some(http) = self.storage.http.clone() {
            match std::sync::Arc::clone(&self.http_spawn_sem).try_acquire_owned() {
                Ok(permit) => {
                    let m = meta.clone();
                    let t = std::sync::Arc::clone(tree);
                    tokio::spawn(async move {
                        let _permit = permit;
                        http.push(&m, &t).await;
                    });
                }
                Err(_) => {
                    let count = self.storage.metrics.inc_http_saturated_drop();
                    if count.is_multiple_of(SATURATED_WARN_EVERY) {
                        tracing::warn!(
                            plugin = "profiler",
                            run_id = %meta.run_id,
                            drops = count,
                            limit = HTTP_SPAWN_LIMIT,
                            "http spawn admission saturated; dropping profile push"
                        );
                    }
                }
            }
        }

        tracing::info!(
            plugin = "profiler",
            run_id = %meta.run_id,
            request_id = view.request_id,
            spans = tree.len(),
            "profile dispatched to storage"
        );
    }

    fn priority(&self) -> Priority {
        // Between apm (-70) and metrics-collection priority band.
        -65
    }
}

fn now_ms() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn build_run_meta(
    view: &PluginCompleteView,
    tree: &std::sync::Arc<crate::profiling::SpanTree>,
    run_id: &str,
) -> storage::RunMeta {
    let trace_id = if tree.trace_id.is_empty() {
        None
    } else {
        Some(tree.trace_id.to_string())
    };
    let span_count = tree.finished.len() as u32;
    let event_count: u32 = tree.finished.iter().map(|s| s.events.len() as u32).sum();
    let error_count = tree
        .finished
        .iter()
        .filter(|s| s.status_code == 2)
        .count()
        .min(u16::MAX as usize) as u16;
    let leaked_count = tree
        .finished
        .iter()
        .filter(|s| s.leaked)
        .count()
        .min(u16::MAX as usize) as u16;

    storage::RunMeta {
        run_id: run_id.to_string(),
        request_id: view.request_id.to_string(),
        trace_id,
        timestamp_ms: now_ms(),
        duration_ms: view.duration.as_millis().min(u32::MAX as u128) as u32,
        method: view.method.to_string(),
        url: view.path.to_string(),
        status: view.status,
        user_agent: None,
        client_ip: Some(view.remote_addr.ip().to_string()),
        // The trigger's ActivationSource is not yet plumbed into the
        // complete view; default to Header until the view is
        // extended to surface it.
        source: ActivationSource::Header,
        span_count,
        event_count,
        error_count,
        leaked_count,
        truncated: false,
        oxphp_version: env!("CARGO_PKG_VERSION").into(),
        formats: vec![],
    }
}

impl Plugin for ProfilerPlugin {
    fn name(&self) -> &'static str {
        "profiler"
    }

    fn version(&self) -> &'static str {
        "0.1.0"
    }

    fn dependencies(&self) -> PluginDeps {
        PluginDeps::default()
    }

    fn init(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        self.config = ProfilerConfig::from_ctx(ctx)?;
        self.enabled = self.config.enabled;

        // Register the OxPHP\Profile\* PHP SDK unconditionally — per spec §6,
        // these functions degrade to safe no-ops when no profile is active so
        // PHP code can call them whether or not the profiler is enabled.
        php_sdk::register_functions(ctx, self.enabled)?;

        // Wire the C bridge to the Rust filter resolver.
        // Once set, observer init calls into Rust whenever a function
        // carries any OxPHP\Profile\* attribute. Filter spec is then
        // cached per-(fn, thread) for hot-path consultation in
        // begin/end. Registered unconditionally (filter behaviour
        // only activates when the user actually applies one of the
        // four filter attributes).
        #[cfg(feature = "php")]
        unsafe {
            crate::php::bindings::oxphp_bridge_set_filter_resolver(Some(
                crate::profiling::filter::oxphp_profiler_resolve_filter,
            ));
        }

        // ── Test-only decorator (feature `decorator-test`) ──
        // Registered before the enabled-guard so it is independent of profiler
        // config: a repeatable, ALL-target decorator whose per-occurrence label
        // is drained back to PHP via `OxPHP\Test\decorator_labels`, letting the
        // integration suite verify per-(name, scope) attribute resolution.
        // Never compiled into shipped images.
        #[cfg(feature = "decorator-test")]
        {
            use crate::plugin::builders::attribute::{
                ATTR_TARGET_CLASS, ATTR_TARGET_FUNCTION, ATTR_TARGET_METHOD,
            };
            use crate::plugin::types::PhpType;

            ctx.register_attribute("OxPHP\\Test\\Mark")
                .target(ATTR_TARGET_FUNCTION | ATTR_TARGET_METHOD | ATTR_TARGET_CLASS)
                .repeatable()
                .optional_param(
                    "label",
                    PhpType::Nullable(Box::new(PhpType::String)),
                    crate::plugin::types::PhpValue::Null,
                )
                .build()?;
            ctx.register_decorator(crate::profiling::decorators::TestMarkDecorator { label: None });
            ctx.function("OxPHP\\Test\\decorator_labels")
                .returns(PhpType::String)
                .handler(|call: &mut crate::bridge::call::NativeCall| {
                    let labels = crate::profiling::decorators::drain_test_decorator_labels();
                    call.ret_str(&labels);
                    Ok(())
                })?;
        }

        if !self.enabled {
            tracing::info!(
                plugin = "profiler",
                "Profiler plugin disabled (PROFILER_ENABLED != true)"
            );
            ctx.expose_config("enabled", false);
            return Ok(());
        }

        ctx.expose_config("enabled", true);
        ctx.expose_config("sample_rate", self.config.sample_rate);
        ctx.expose_config("auth_token_configured", self.config.auth_token.is_some());
        ctx.expose_config("exclude_paths", self.config.exclude_patterns.clone());

        // Apply the per-request span cap to the C observer. Runs
        // only when the profiler is enabled — otherwise no spans are
        // ever captured and the cap is moot. 0 → unlimited (mapped
        // to UINT32_MAX inside the bridge).
        #[cfg(feature = "php")]
        unsafe {
            crate::php::bindings::oxphp_bridge_set_profiler_max_spans(self.config.max_spans);
        }

        ctx.on_request(ProfilerRequestHandler {
            config: self.config.clone(),
        });

        // Build the storage handle (cache always; disk + http
        // optional) and wire the complete handler to it.
        // Retention background task runs only when disk is enabled.
        let output_formats: Vec<storage::OutputFormat> = self
            .config
            .output_formats
            .iter()
            .filter_map(|s| storage::OutputFormat::from_str_opt(s))
            .collect();

        let metrics = storage::StorageMetrics::new();

        // One lock shared by the DiskWriter append path, the
        // retention sweep, and the DELETE route so all three are
        // totally ordered against each other on index.json.
        let index_lock = std::sync::Arc::new(tokio::sync::Mutex::new(()));

        let disk = if !self.config.output_dir.as_os_str().is_empty() {
            let rate = self.config.disk_max_per_sec.max(1) as f64;
            Some(storage::DiskWriter::with_index_lock(
                self.config.output_dir.clone(),
                rate,
                rate, // capacity == burst budget
                std::sync::Arc::clone(&metrics),
                std::sync::Arc::clone(&index_lock),
            ))
        } else {
            None
        };

        let http = self.config.export_url.as_ref().and_then(|url| {
            let format = storage::OutputFormat::from_str_opt(&self.config.export_format)
                .unwrap_or(storage::OutputFormat::Xhprof);
            let token = self
                .config
                .export_auth_token
                .as_ref()
                .map(|t| t.to_string());
            match storage::HttpPusher::new(
                url.to_string(),
                format,
                token,
                self.config.export_xhgui,
                std::sync::Arc::clone(&metrics),
            ) {
                Ok(p) => Some(p),
                Err(e) => {
                    tracing::error!(
                        plugin = "profiler",
                        error = %e,
                        "failed to build reqwest client; HTTP push disabled"
                    );
                    None
                }
            }
        });

        let storage = std::sync::Arc::new(storage::Storage {
            cache: std::sync::Arc::new(storage::ProfileCache::new(
                self.config.retention_count as usize,
            )),
            disk: disk.map(std::sync::Arc::new),
            http: http.map(std::sync::Arc::new),
            metrics,
        });

        if storage.disk.is_some() {
            storage::retention::spawn(
                self.config.output_dir.clone(),
                self.config.retention_count as usize,
                std::sync::Arc::clone(&index_lock),
            );
        }

        let config_view = serde_json::json!({
            "enabled": true,
            "auth_token_configured": self.config.auth_token.is_some(),
            "sample_rate": self.config.sample_rate,
            "internal": self.config.internal,
            "max_spans": self.config.max_spans,
            "max_depth": self.config.max_depth,
            "output_dir": self.config.output_dir.display().to_string(),
            "output_formats": self.config.output_formats.clone(),
            "disk_max_per_sec": self.config.disk_max_per_sec,
            "retention_count": self.config.retention_count,
            "export_url_configured": self.config.export_url.is_some(),
            "export_format": self.config.export_format.clone(),
            "export_xhgui": self.config.export_xhgui,
        });

        routes::register(
            ctx,
            std::sync::Arc::clone(&storage),
            self.config.auth_token.clone(),
            config_view,
        );

        // Register the Prometheus collector so `/metrics` includes the
        // `oxphp_profiler_*` counters + the `in_memory_runs` gauge
        // (read live from the cache at collection time).
        let storage_for_metrics = std::sync::Arc::clone(&storage);
        ctx.register_metrics(move |out: &mut String| {
            let in_memory = storage_for_metrics.cache.len() as u64;
            storage_for_metrics.metrics.collect(out, in_memory);
        });

        ctx.on_complete(ProfilerCompleteHandler {
            storage,
            output_formats,
            xhgui_envelope: self.config.export_xhgui,
            disk_spawn_sem: std::sync::Arc::new(tokio::sync::Semaphore::new(DISK_SPAWN_LIMIT)),
            http_spawn_sem: std::sync::Arc::new(tokio::sync::Semaphore::new(HTTP_SPAWN_LIMIT)),
        });

        // Register the seven OxPHP\Profile\* PHP attributes from spec §6:
        //   - Decorator-based (affect already-created spans):
        //     Mark, SlowThreshold, MemoryThreshold
        //   - Observer-filter (affect span creation):
        //     Profile, Exclude, Sample, Tag
        {
            use crate::plugin::builders::attribute::{
                ATTR_TARGET_CLASS, ATTR_TARGET_FUNCTION, ATTR_TARGET_METHOD,
            };
            use crate::plugin::types::{PhpType, PhpValue};

            // ── Decorators ─────────────────────────────────
            ctx.register_attribute("OxPHP\\Profile\\Mark")
                .target(ATTR_TARGET_FUNCTION | ATTR_TARGET_METHOD)
                .optional_param(
                    "label",
                    PhpType::Nullable(Box::new(PhpType::String)),
                    PhpValue::Null,
                )
                .build()?;

            ctx.register_attribute("OxPHP\\Profile\\SlowThreshold")
                .target(ATTR_TARGET_FUNCTION | ATTR_TARGET_METHOD)
                .param("ms", PhpType::Int)
                .build()?;

            ctx.register_attribute("OxPHP\\Profile\\MemoryThreshold")
                .target(ATTR_TARGET_FUNCTION | ATTR_TARGET_METHOD)
                .param("kb", PhpType::Int)
                .build()?;

            // ── Observer-filter attributes ─────────────────
            ctx.register_attribute("OxPHP\\Profile\\Profile")
                .target(ATTR_TARGET_FUNCTION | ATTR_TARGET_METHOD | ATTR_TARGET_CLASS)
                .build()?;

            ctx.register_attribute("OxPHP\\Profile\\Exclude")
                .target(ATTR_TARGET_FUNCTION | ATTR_TARGET_METHOD | ATTR_TARGET_CLASS)
                .build()?;

            ctx.register_attribute("OxPHP\\Profile\\Sample")
                .target(ATTR_TARGET_FUNCTION | ATTR_TARGET_METHOD)
                .param("rate", PhpType::Float)
                .build()?;

            ctx.register_attribute("OxPHP\\Profile\\Tag")
                .target(ATTR_TARGET_FUNCTION | ATTR_TARGET_METHOD | ATTR_TARGET_CLASS)
                .repeatable() // multiple #[Tag] on one target accumulate
                .param("key", PhpType::String)
                .param("value", PhpType::String)
                .build()?;
        }
        ctx.register_decorator(crate::profiling::decorators::MarkDecorator { label: None });
        ctx.register_decorator(crate::profiling::decorators::SlowThresholdDecorator { ms: 100 });
        ctx.register_decorator(crate::profiling::decorators::MemoryThresholdDecorator { kb: 64 });

        if self.config.auth_token.is_none() {
            tracing::warn!(
                plugin = "profiler",
                "PROFILER_AUTH_TOKEN not set — any non-empty token will activate profiling \
                 (OK for dev, DANGER in prod)"
            );
        }

        tracing::info!(
            plugin = "profiler",
            sample_rate = self.config.sample_rate,
            internal = self.config.internal,
            max_spans = self.config.max_spans,
            max_depth = self.config.max_depth,
            auth_token_configured = self.config.auth_token.is_some(),
            export_url_configured = self.config.export_url.is_some(),
            export_xhgui = self.config.export_xhgui,
            output_formats = ?self.config.output_formats,
            "Profiler plugin initialized"
        );

        Ok(())
    }

    fn shutdown(&mut self) {
        if !self.enabled {
            return;
        }
        tracing::info!(plugin = "profiler", "Profiler plugin shutdown complete");
    }

    fn health(&self) -> PluginHealth {
        PluginHealth::Ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventDispatcher;
    use crate::plugin::context::PluginDecoratorDef;
    use crate::plugin::handler::{PluginInternalHandler, PluginMetricsCollector};
    use crate::plugin::php::PluginNativeFunctionDef;
    use std::collections::HashMap;

    fn try_init_profiler_plugin(
        plugin: &mut ProfilerPlugin,
    ) -> Result<HashMap<String, serde_json::Value>, crate::plugin::PluginError> {
        let mut dispatcher = EventDispatcher::new();
        let mut services: HashMap<String, Box<dyn std::any::Any + Send + Sync>> = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics_collectors: Vec<Box<dyn PluginMetricsCollector>> = Vec::new();
        let mut internal_routes: HashMap<String, Box<dyn PluginInternalHandler>> = HashMap::new();
        let mut internal_route_prefixes: Vec<(String, Box<dyn PluginInternalHandler>)> = Vec::new();
        let mut native_php_functions: Vec<PluginNativeFunctionDef> = Vec::new();
        let mut decorators: Vec<PluginDecoratorDef> = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();
        let mut core_flags = HashMap::new();

        let mut ctx = PluginContext::new(
            "profiler".into(),
            "__oxp_profiler_".into(),
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
        let result = plugin.init(&mut ctx);
        drop(ctx);
        result.map(|()| config_values)
    }

    fn init_profiler_plugin(plugin: &mut ProfilerPlugin) -> HashMap<String, serde_json::Value> {
        try_init_profiler_plugin(plugin).unwrap()
    }

    /// Serialises every test that reads or writes `PROFILER_*` env vars.
    /// `cargo test` runs tests in parallel, and the strict bool parser now
    /// rejects garbage values (e.g. `PROFILER_INTERNAL=ture`) — without this
    /// lock a test that *sets* a bad value would make every other concurrent
    /// `init_profiler_plugin` panic on `unwrap()`.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Bool env vars the profiler config reads. These are the only ones
    /// that can reject and abort `init`, so we clear them on entry and
    /// restore them on exit to keep tests hermetic.
    const BOOL_VARS: &[&str] = &[
        "PROFILER_ENABLED",
        "PROFILER_INTERNAL",
        "PROFILER_EXPORT_XHGUI",
    ];

    /// Run `f` with the given env-var overrides applied, holding `ENV_LOCK`
    /// for the entire duration. All [`BOOL_VARS`] that aren't explicitly
    /// listed in `overrides` are cleared, so a leaked value from another
    /// test or the host environment cannot bleed in.
    fn with_env<F, R>(overrides: &[(&str, &str)], f: F) -> R
    where
        F: FnOnce() -> R,
    {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let prev: Vec<(String, Option<String>)> = BOOL_VARS
            .iter()
            .map(|k| (k.to_string(), std::env::var(k).ok()))
            .collect();
        for k in BOOL_VARS {
            std::env::remove_var(k);
        }
        for (k, v) in overrides {
            std::env::set_var(k, v);
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        for (k, prev_val) in prev {
            match prev_val {
                Some(v) => std::env::set_var(&k, v),
                None => std::env::remove_var(&k),
            }
        }
        match result {
            Ok(r) => r,
            Err(e) => std::panic::resume_unwind(e),
        }
    }

    #[test]
    fn test_profiler_plugin_disabled_by_default() {
        with_env(&[], || {
            let mut plugin = ProfilerPlugin::new();
            let config = init_profiler_plugin(&mut plugin);
            assert_eq!(plugin.name(), "profiler");
            assert_eq!(plugin.version(), "0.1.0");
            assert!(!plugin.enabled);
            assert_eq!(config.get("enabled"), Some(&serde_json::json!(false)));
            assert_eq!(plugin.health(), PluginHealth::Ok);
        });
    }

    #[test]
    fn test_profiler_plugin_has_no_required_dependencies() {
        let plugin = ProfilerPlugin::new();
        let deps = plugin.dependencies();
        assert!(deps.required.is_empty());
        assert!(deps.optional.is_empty());
        assert!(deps.services.is_empty());
    }

    #[test]
    fn test_profiler_plugin_shutdown_disabled() {
        let mut plugin = ProfilerPlugin::new();
        plugin.shutdown();
    }

    #[test]
    fn test_profiler_validates_other_bools_when_disabled() {
        // PROFILER_ENABLED=false used to skip parsing of every other bool —
        // a typo like PROFILER_INTERNAL=ture would only surface in prod the
        // day someone flipped PROFILER_ENABLED=true. With the unified policy
        // the typo must fail at startup regardless of `enabled`.
        let err = with_env(
            &[("PROFILER_ENABLED", "false"), ("PROFILER_INTERNAL", "ture")],
            || {
                let mut plugin = ProfilerPlugin::new();
                try_init_profiler_plugin(&mut plugin)
                    .expect_err("PROFILER_INTERNAL=ture must fail even when disabled")
            },
        );

        let msg = err.to_string();
        assert!(
            msg.contains("PROFILER_INTERNAL"),
            "error should name the offending var, got: {msg}"
        );
        assert!(
            msg.contains("ture"),
            "error should echo the value, got: {msg}"
        );
    }
}
