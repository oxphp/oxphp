use std::any::Any;
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

use crate::events::EventDispatcher;

use super::builders::definitions::*;
use super::context::{PluginContext, PluginDecoratorDef};
use super::handler::{PluginInternalHandler, PluginInternalRequest, PluginMetricsCollector};
use super::php::PluginNativeFunctionDef;
use super::{Plugin, PluginError, PluginHealth};
use crate::types::ResponseBody;

/// Loads, initializes, and manages plugin lifecycle.
///
/// After `init_all` the manager is typically wrapped in `Arc` and shared
/// across request handlers. Because request paths (`/health`, `/metrics`)
/// and the shutdown path need access through the shared `Arc`, the plugin
/// vec lives behind a `Mutex`. The lock is uncontended in practice —
/// `/health` takes it briefly, `shutdown_all` takes it once.
pub struct PluginManager {
    plugins: Mutex<Vec<Box<dyn Plugin>>>,
    services: HashMap<String, Box<dyn Any + Send + Sync>>,
    config_values: HashMap<String, HashMap<String, serde_json::Value>>,
    metrics_collectors: Vec<Box<dyn PluginMetricsCollector>>,
    internal_routes: HashMap<String, Box<dyn PluginInternalHandler>>,
    internal_route_prefixes: Vec<(String, Box<dyn PluginInternalHandler>)>,
    native_php_functions: Vec<PluginNativeFunctionDef>,
    decorators: Vec<PluginDecoratorDef>,
    php_classes: Vec<PhpClassDef>,
    php_interfaces: Vec<PhpInterfaceDef>,
    php_enums: Vec<PhpEnumDef>,
    php_attributes: Vec<PhpAttributeDef>,
    php_functions: Vec<PhpFunctionDef>,
    /// Flags set by plugins during `init()` to signal core config changes.
    /// Read by `main()` after `init_all()` to patch `Config` before the
    /// runtime starts. Replaces the old pattern of `env::set_var` side-effects.
    core_flags: HashMap<String, String>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: Mutex::new(Vec::new()),
            services: HashMap::new(),
            config_values: HashMap::new(),
            metrics_collectors: Vec::new(),
            internal_routes: HashMap::new(),
            internal_route_prefixes: Vec::new(),
            native_php_functions: Vec::new(),
            decorators: Vec::new(),
            php_classes: Vec::new(),
            php_interfaces: Vec::new(),
            php_enums: Vec::new(),
            php_attributes: Vec::new(),
            php_functions: Vec::new(),
            core_flags: HashMap::new(),
        }
    }

    /// Read a core flag set by a plugin during `init()`.
    pub fn core_flag(&self, key: &str) -> Option<&str> {
        self.core_flags.get(key).map(|s| s.as_str())
    }

    /// Register a plugin.
    pub fn add(&mut self, plugin: Box<dyn Plugin>) {
        self.plugins.get_mut().expect("plugins mutex").push(plugin);
    }

    /// Initialize all plugins in dependency order.
    /// Registers plugin handlers on the dispatcher (must be called BEFORE freeze).
    /// Reorders the internal plugin vec to match topological order so that
    /// `on_ready_all()` runs in init order and `shutdown_all()` in reverse.
    pub fn init_all(&mut self, dispatcher: &mut EventDispatcher) -> Result<(), PluginError> {
        let plugins = self.plugins.get_mut().expect("plugins mutex");
        let order = Self::resolve_init_order(plugins)?;

        // Reorder plugins to match topological init order.
        let mut slots: Vec<Option<Box<dyn Plugin>>> = plugins.drain(..).map(Some).collect();
        for idx in order {
            plugins.push(slots[idx].take().unwrap());
        }

        for plugin in plugins.iter_mut() {
            let name = plugin.name().to_string();
            let mut plugin_config = HashMap::new();
            let mut ctx = PluginContext::new(
                name.clone(),
                format!("__oxp_{}_", name),
                dispatcher,
                &mut self.services,
                &mut plugin_config,
                &mut self.metrics_collectors,
                &mut self.internal_routes,
                &mut self.internal_route_prefixes,
                &mut self.native_php_functions,
                &mut self.decorators,
                &mut self.php_classes,
                &mut self.php_interfaces,
                &mut self.php_enums,
                &mut self.php_attributes,
                &mut self.php_functions,
                &mut self.core_flags,
            );

            plugin.init(&mut ctx)?;
            if !plugin_config.is_empty() {
                self.config_values.insert(name.clone(), plugin_config);
            }
            tracing::info!(name = %name, version = plugin.version(), "Plugin initialized");
        }

        // Sort prefix registry longest-first so longest-prefix-wins dispatch
        // is a simple linear scan. Called once after every plugin has had a
        // chance to register its prefixes.
        self.internal_route_prefixes
            .sort_by_key(|p| std::cmp::Reverse(p.0.len()));

        Ok(())
    }

    /// Call on_ready() for all plugins (after init_all + dispatcher.freeze).
    /// Catches panics so a single misbehaving plugin cannot poison the mutex
    /// and take down `/health` or shutdown.
    pub fn on_ready_all(&self) {
        let plugins = Self::lock_plugins(&self.plugins);
        for plugin in plugins.iter() {
            let name = plugin.name();
            if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                plugin.on_ready();
            }))
            .is_err()
            {
                tracing::error!(name, "Plugin panicked during on_ready");
            }
        }
    }

    /// Shutdown all plugins in reverse init order.
    /// Catches panics and logs errors to ensure all plugins get a chance to shut down.
    /// Idempotent: after the first call the plugin vec is empty, so subsequent
    /// calls are no-ops.
    ///
    /// Each plugin is popped from the vec, `shutdown()` is called under
    /// `catch_unwind`, and then the plugin is dropped **outside** the
    /// `catch_unwind` block. If a `Drop` impl panics, the process aborts
    /// (standard Rust double-panic semantics). Plugin authors must ensure
    /// their `Drop` is infallible — the same contract Rust imposes everywhere.
    pub fn shutdown_all(&self) {
        let mut plugins = Self::lock_plugins(&self.plugins);
        let mut failures = 0usize;
        // Collect plugins in reverse order first, then release the mutex
        // guard before running shutdown+drop so that neither runs under lock.
        let reversed: Vec<_> = std::iter::from_fn(|| plugins.pop()).collect();
        drop(plugins);

        for mut plugin in reversed {
            let name = plugin.name().to_string();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                plugin.shutdown();
            })) {
                Ok(()) => {
                    tracing::info!(name = %name, "Plugin shutdown");
                }
                Err(_) => {
                    failures += 1;
                    tracing::error!(name = %name, "Plugin panicked during shutdown");
                }
            }
            // plugin dropped here, outside the mutex guard
        }
        if failures > 0 {
            tracing::warn!(failures, "Some plugins failed during shutdown");
        }
    }

    /// Get all plugin health statuses (for /health endpoint).
    pub fn health_all(&self) -> Vec<(String, PluginHealth)> {
        let plugins = Self::lock_plugins(&self.plugins);
        plugins
            .iter()
            .map(|p| (p.name().to_string(), p.health()))
            .collect()
    }

    /// Acquire the plugins mutex, recovering from poisoning.
    /// Poisoning can happen if `name()` or `health()` panic inside
    /// `health_all` (called under the guard without `catch_unwind`).
    /// We always recover because the Vec itself is structurally valid.
    fn lock_plugins(
        mutex: &Mutex<Vec<Box<dyn Plugin>>>,
    ) -> std::sync::MutexGuard<'_, Vec<Box<dyn Plugin>>> {
        match mutex.lock() {
            Ok(g) => g,
            Err(poisoned) => {
                tracing::warn!("plugins mutex poisoned, recovering");
                poisoned.into_inner()
            }
        }
    }

    /// Get plugin config values for /config endpoint.
    pub fn config_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.config_values).unwrap_or_default()
    }

    /// Collect Prometheus metrics from all plugins. Appends to output.
    pub fn collect_metrics(&self, output: &mut String) {
        for collector in &self.metrics_collectors {
            collector.collect(output);
        }
    }

    /// Route an internal server request to a plugin handler.
    /// Exact matches win; otherwise fall back to the longest registered
    /// prefix (the prefix Vec is sorted descending by length in `init_all`).
    /// Returns None if no plugin owns this path.
    pub fn handle_internal_route(
        &self,
        req: &PluginInternalRequest,
    ) -> Option<http::Response<ResponseBody>> {
        if let Some(h) = self.internal_routes.get(req.path) {
            return Some(h.handle(req));
        }
        for (prefix, h) in &self.internal_route_prefixes {
            if req.path.starts_with(prefix.as_str()) {
                return Some(h.handle(req));
            }
        }
        None
    }

    /// Take native plugin PHP function definitions (empties the internal vec).
    /// Call after init_all(), before wrapping manager in Arc.
    pub fn take_native_php_functions(&mut self) -> Vec<PluginNativeFunctionDef> {
        std::mem::take(&mut self.native_php_functions)
    }

    /// Take decorator definitions (empties the internal vec).
    /// Call after init_all(), before wrapping manager in Arc.
    pub fn take_decorators(&mut self) -> Vec<PluginDecoratorDef> {
        std::mem::take(&mut self.decorators)
    }

    /// Take all PHP type/function definitions contributed by plugins.
    /// Call after init_all(), before wrapping manager in Arc.
    pub fn take_php_definitions(&mut self) -> PhpDefinitions {
        PhpDefinitions {
            classes: std::mem::take(&mut self.php_classes),
            interfaces: std::mem::take(&mut self.php_interfaces),
            enums: std::mem::take(&mut self.php_enums),
            attributes: std::mem::take(&mut self.php_attributes),
            functions: std::mem::take(&mut self.php_functions),
        }
    }

    /// Validate dependencies and return topological init order (Kahn's algorithm).
    /// Checks that all required deps exist while building the adjacency graph.
    fn resolve_init_order(plugins: &[Box<dyn Plugin>]) -> Result<Vec<usize>, PluginError> {
        let name_to_idx: HashMap<&str, usize> = plugins
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name(), i))
            .collect();

        let n = plugins.len();
        let mut in_degree = vec![0usize; n];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];

        for (idx, plugin) in plugins.iter().enumerate() {
            for dep in &plugin.dependencies().required {
                let &dep_idx = name_to_idx.get(dep).ok_or_else(|| {
                    PluginError::DependencyMissing(format!(
                        "Plugin '{}' requires '{}', which is not loaded",
                        plugin.name(),
                        dep
                    ))
                })?;
                adj[dep_idx].push(idx);
                in_degree[idx] += 1;
            }
        }

        let mut queue: VecDeque<usize> = in_degree
            .iter()
            .enumerate()
            .filter(|(_, &d)| d == 0)
            .map(|(i, _)| i)
            .collect();

        let mut sorted = Vec::new();
        while let Some(idx) = queue.pop_front() {
            sorted.push(idx);
            for &next in &adj[idx] {
                in_degree[next] -= 1;
                if in_degree[next] == 0 {
                    queue.push_back(next);
                }
            }
        }

        if sorted.len() != n {
            return Err(PluginError::Config("Circular dependency detected".into()));
        }
        Ok(sorted)
    }
}

impl Default for PluginManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventDispatcher;
    use crate::plugin::{PluginDeps, PluginError, PluginHealth};
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    // ── Test plugin helpers ──

    struct TestPlugin {
        _name: &'static str,
        _version: &'static str,
        deps: PluginDeps,
        health: PluginHealth,
        init_called: Arc<AtomicBool>,
        shutdown_called: Arc<AtomicBool>,
    }

    impl TestPlugin {
        fn simple(name: &'static str) -> Self {
            Self {
                _name: name,
                _version: "1.0.0",
                deps: PluginDeps::default(),
                health: PluginHealth::Ok,
                init_called: Arc::new(AtomicBool::new(false)),
                shutdown_called: Arc::new(AtomicBool::new(false)),
            }
        }

        fn with_deps(name: &'static str, required: Vec<&'static str>) -> Self {
            Self {
                _name: name,
                _version: "1.0.0",
                deps: PluginDeps {
                    required,
                    ..Default::default()
                },
                health: PluginHealth::Ok,
                init_called: Arc::new(AtomicBool::new(false)),
                shutdown_called: Arc::new(AtomicBool::new(false)),
            }
        }
    }

    impl Plugin for TestPlugin {
        fn name(&self) -> &'static str {
            self._name
        }
        fn version(&self) -> &'static str {
            self._version
        }
        fn init(&mut self, _ctx: &mut PluginContext) -> Result<(), PluginError> {
            self.init_called.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn shutdown(&mut self) {
            self.shutdown_called.store(true, Ordering::SeqCst);
        }
        fn dependencies(&self) -> PluginDeps {
            PluginDeps {
                required: self.deps.required.clone(),
                optional: self.deps.optional.clone(),
                services: self.deps.services.clone(),
            }
        }
        fn health(&self) -> PluginHealth {
            self.health
        }
    }

    // ── Tests ──

    #[test]
    fn test_plugin_init_and_shutdown() {
        let init_called = Arc::new(AtomicBool::new(false));
        let shutdown_called = Arc::new(AtomicBool::new(false));

        let mut plugin = TestPlugin::simple("test");
        plugin.init_called = Arc::clone(&init_called);
        plugin.shutdown_called = Arc::clone(&shutdown_called);

        let mut manager = PluginManager::new();
        manager.add(Box::new(plugin));

        let mut dispatcher = EventDispatcher::new();
        assert!(manager.init_all(&mut dispatcher).is_ok());
        assert!(init_called.load(Ordering::SeqCst));

        manager.shutdown_all();
        assert!(shutdown_called.load(Ordering::SeqCst));
    }

    #[test]
    fn test_no_plugins() {
        let mut manager = PluginManager::new();
        let mut dispatcher = EventDispatcher::new();
        assert!(manager.init_all(&mut dispatcher).is_ok());
        manager.shutdown_all();
        assert!(manager.health_all().is_empty());
    }

    #[test]
    fn test_circular_dependency_detected() {
        let mut manager = PluginManager::new();
        manager.add(Box::new(TestPlugin::with_deps("a", vec!["b"])));
        manager.add(Box::new(TestPlugin::with_deps("b", vec!["a"])));

        let mut dispatcher = EventDispatcher::new();
        let result = manager.init_all(&mut dispatcher);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Circular dependency"));
    }

    #[test]
    fn test_missing_dependency() {
        let mut manager = PluginManager::new();
        manager.add(Box::new(TestPlugin::with_deps("a", vec!["missing"])));

        let mut dispatcher = EventDispatcher::new();
        let result = manager.init_all(&mut dispatcher);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("missing"));
    }

    #[test]
    fn test_dependency_order() {
        // c depends on b, b depends on a. Init order should be a, b, c.
        static ORDER: AtomicUsize = AtomicUsize::new(0);

        struct OrderPlugin {
            _name: &'static str,
            deps: Vec<&'static str>,
            expected_order: usize,
        }

        impl Plugin for OrderPlugin {
            fn name(&self) -> &'static str {
                self._name
            }
            fn version(&self) -> &'static str {
                "1.0"
            }
            fn init(&mut self, _ctx: &mut PluginContext) -> Result<(), PluginError> {
                let actual = ORDER.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    actual, self.expected_order,
                    "Plugin '{}' initialized in wrong order",
                    self._name
                );
                Ok(())
            }
            fn dependencies(&self) -> PluginDeps {
                PluginDeps {
                    required: self.deps.clone(),
                    ..Default::default()
                }
            }
        }

        ORDER.store(0, Ordering::SeqCst);

        let mut manager = PluginManager::new();
        // Register in reverse order to prove sorting works
        manager.add(Box::new(OrderPlugin {
            _name: "c",
            deps: vec!["b"],
            expected_order: 2,
        }));
        manager.add(Box::new(OrderPlugin {
            _name: "a",
            deps: vec![],
            expected_order: 0,
        }));
        manager.add(Box::new(OrderPlugin {
            _name: "b",
            deps: vec!["a"],
            expected_order: 1,
        }));

        let mut dispatcher = EventDispatcher::new();
        assert!(manager.init_all(&mut dispatcher).is_ok());
    }

    #[test]
    fn test_health_all() {
        let mut manager = PluginManager::new();

        let mut p1 = TestPlugin::simple("healthy");
        p1.health = PluginHealth::Ok;
        let mut p2 = TestPlugin::simple("degraded");
        p2.health = PluginHealth::Degraded;
        let mut p3 = TestPlugin::simple("failed");
        p3.health = PluginHealth::Failed;

        manager.add(Box::new(p1));
        manager.add(Box::new(p2));
        manager.add(Box::new(p3));

        let mut dispatcher = EventDispatcher::new();
        manager.init_all(&mut dispatcher).unwrap();

        let health = manager.health_all();
        assert_eq!(health.len(), 3);
        assert_eq!(health[0], ("healthy".to_string(), PluginHealth::Ok));
        assert_eq!(health[1], ("degraded".to_string(), PluginHealth::Degraded));
        assert_eq!(health[2], ("failed".to_string(), PluginHealth::Failed));
    }

    #[test]
    fn test_config_json_empty() {
        let manager = PluginManager::new();
        let json = manager.config_json();
        assert_eq!(json, serde_json::json!({}));
    }

    #[test]
    fn test_collect_metrics() {
        let mut manager = PluginManager::new();

        // Simulate a plugin that registered a metrics collector
        manager
            .metrics_collectors
            .push(Box::new(|output: &mut String| {
                output.push_str("plugin_test_counter 42\n");
            }));

        let mut output = String::new();
        manager.collect_metrics(&mut output);
        assert_eq!(output, "plugin_test_counter 42\n");
    }

    #[test]
    fn test_handle_internal_route_none() {
        let manager = PluginManager::new();
        let headers = http::HeaderMap::new();
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__test/status",
            headers: &headers,
            query: None,
        };
        assert!(manager.handle_internal_route(&req).is_none());
    }

    #[test]
    fn test_shutdown_reverse_order() {
        static SHUTDOWN_ORDER: AtomicUsize = AtomicUsize::new(0);

        struct ShutdownPlugin {
            _name: &'static str,
            expected_shutdown_order: usize,
        }

        impl Plugin for ShutdownPlugin {
            fn name(&self) -> &'static str {
                self._name
            }
            fn version(&self) -> &'static str {
                "1.0"
            }
            fn init(&mut self, _ctx: &mut PluginContext) -> Result<(), PluginError> {
                Ok(())
            }
            fn shutdown(&mut self) {
                let actual = SHUTDOWN_ORDER.fetch_add(1, Ordering::SeqCst);
                assert_eq!(
                    actual, self.expected_shutdown_order,
                    "Plugin '{}' shutdown in wrong order",
                    self._name
                );
            }
        }

        SHUTDOWN_ORDER.store(0, Ordering::SeqCst);

        let mut manager = PluginManager::new();
        manager.add(Box::new(ShutdownPlugin {
            _name: "first",
            expected_shutdown_order: 1, // shutdown last (reverse of add order)
        }));
        manager.add(Box::new(ShutdownPlugin {
            _name: "second",
            expected_shutdown_order: 0, // shutdown first
        }));

        let mut dispatcher = EventDispatcher::new();
        manager.init_all(&mut dispatcher).unwrap();
        manager.shutdown_all();
    }

    #[test]
    fn test_prefix_route_matches_path_below() {
        use crate::plugin::handler::PluginInternalRequest;
        use bytes::Bytes;

        let mut manager = PluginManager::new();
        manager.internal_route_prefixes.push((
            "/__test/runs/".into(),
            Box::new(|_req: &PluginInternalRequest| {
                http::Response::builder()
                    .status(http::StatusCode::OK)
                    .body(crate::types::full_body(Bytes::from_static(b"match")))
                    .unwrap()
            }),
        ));
        let headers = http::HeaderMap::new();
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__test/runs/abc-123",
            headers: &headers,
            query: None,
        };
        let resp = manager.handle_internal_route(&req);
        assert!(resp.is_some());
        assert_eq!(resp.unwrap().status(), http::StatusCode::OK);
    }

    #[test]
    fn test_exact_route_wins_over_prefix() {
        use crate::plugin::handler::PluginInternalRequest;
        use bytes::Bytes;

        let mut manager = PluginManager::new();
        manager.internal_routes.insert(
            "/__test/runs".into(),
            Box::new(|_req: &PluginInternalRequest| {
                http::Response::builder()
                    .status(http::StatusCode::OK)
                    .header("x-match", "exact")
                    .body(crate::types::full_body(Bytes::new()))
                    .unwrap()
            }),
        );
        manager.internal_route_prefixes.push((
            "/__test/".into(),
            Box::new(|_req: &PluginInternalRequest| {
                http::Response::builder()
                    .status(http::StatusCode::OK)
                    .header("x-match", "prefix")
                    .body(crate::types::full_body(Bytes::new()))
                    .unwrap()
            }),
        ));
        let headers = http::HeaderMap::new();
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__test/runs",
            headers: &headers,
            query: None,
        };
        let resp = manager.handle_internal_route(&req).expect("matched");
        assert_eq!(resp.headers().get("x-match").unwrap(), "exact");
    }

    #[test]
    fn test_longest_prefix_wins() {
        use crate::plugin::handler::PluginInternalRequest;
        use bytes::Bytes;

        let mut manager = PluginManager::new();
        manager.internal_route_prefixes.push((
            "/__test/runs/".into(),
            Box::new(|_req: &PluginInternalRequest| {
                http::Response::builder()
                    .status(http::StatusCode::OK)
                    .header("x-match", "runs")
                    .body(crate::types::full_body(Bytes::new()))
                    .unwrap()
            }),
        ));
        manager.internal_route_prefixes.push((
            "/__test/".into(),
            Box::new(|_req: &PluginInternalRequest| {
                http::Response::builder()
                    .status(http::StatusCode::OK)
                    .header("x-match", "ns")
                    .body(crate::types::full_body(Bytes::new()))
                    .unwrap()
            }),
        ));
        manager
            .internal_route_prefixes
            .sort_by_key(|p| std::cmp::Reverse(p.0.len()));
        let headers = http::HeaderMap::new();
        let req = PluginInternalRequest {
            method: &http::Method::GET,
            path: "/__test/runs/abc",
            headers: &headers,
            query: None,
        };
        let resp = manager.handle_internal_route(&req).expect("matched");
        assert_eq!(resp.headers().get("x-match").unwrap(), "runs");
    }
}
