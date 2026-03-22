use std::any::Any;
use std::collections::{HashMap, VecDeque};

use crate::events::EventDispatcher;

use super::context::{PluginContext, PluginDecoratorDef};
use super::handler::{PluginInternalHandler, PluginInternalRequest, PluginMetricsCollector};
use super::php::PluginNativeFunctionDef;
use super::{Plugin, PluginError, PluginHealth};
use crate::types::ResponseBody;

/// Loads, initializes, and manages plugin lifecycle.
pub struct PluginManager {
    plugins: Vec<Box<dyn Plugin>>,
    services: HashMap<String, Box<dyn Any + Send + Sync>>,
    config_values: HashMap<String, HashMap<String, serde_json::Value>>,
    metrics_collectors: Vec<Box<dyn PluginMetricsCollector>>,
    internal_routes: HashMap<String, Box<dyn PluginInternalHandler>>,
    native_php_functions: Vec<PluginNativeFunctionDef>,
    decorators: Vec<PluginDecoratorDef>,
}

impl PluginManager {
    pub fn new() -> Self {
        Self {
            plugins: Vec::new(),
            services: HashMap::new(),
            config_values: HashMap::new(),
            metrics_collectors: Vec::new(),
            internal_routes: HashMap::new(),
            native_php_functions: Vec::new(),
            decorators: Vec::new(),
        }
    }

    /// Register a plugin.
    pub fn add(&mut self, plugin: Box<dyn Plugin>) {
        self.plugins.push(plugin);
    }

    /// Initialize all plugins in dependency order.
    /// Registers plugin handlers on the dispatcher (must be called BEFORE freeze).
    /// Reorders the internal plugin vec to match topological order so that
    /// `on_ready_all()` runs in init order and `shutdown_all()` in reverse.
    pub fn init_all(&mut self, dispatcher: &mut EventDispatcher) -> Result<(), PluginError> {
        let order = self.resolve_init_order()?;

        // Reorder plugins to match topological init order.
        let mut slots: Vec<Option<Box<dyn Plugin>>> = self.plugins.drain(..).map(Some).collect();
        for idx in order {
            self.plugins.push(slots[idx].take().unwrap());
        }

        for i in 0..self.plugins.len() {
            let plugin = &mut self.plugins[i];
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
                &mut self.native_php_functions,
                &mut self.decorators,
            );

            plugin.init(&mut ctx)?;
            if !plugin_config.is_empty() {
                self.config_values.insert(name.clone(), plugin_config);
            }
            tracing::info!(name = %name, version = plugin.version(), "Plugin initialized");
        }

        Ok(())
    }

    /// Call on_ready() for all plugins (after init_all + dispatcher.freeze).
    pub fn on_ready_all(&self) {
        for plugin in &self.plugins {
            plugin.on_ready();
        }
    }

    /// Shutdown all plugins in reverse init order.
    /// Catches panics and logs errors to ensure all plugins get a chance to shut down.
    pub fn shutdown_all(&self) {
        let mut failures = 0usize;
        for plugin in self.plugins.iter().rev() {
            let name = plugin.name();
            match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                plugin.shutdown();
            })) {
                Ok(()) => {
                    tracing::info!(name, "Plugin shutdown");
                }
                Err(_) => {
                    failures += 1;
                    tracing::error!(name, "Plugin panicked during shutdown");
                }
            }
        }
        if failures > 0 {
            tracing::warn!(failures, "Some plugins failed during shutdown");
        }
    }

    /// Get all plugin health statuses (for /health endpoint).
    pub fn health_all(&self) -> Vec<(&str, PluginHealth)> {
        self.plugins
            .iter()
            .map(|p| (p.name(), p.health()))
            .collect()
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
    /// Returns None if no plugin owns this path.
    pub fn handle_internal_route(
        &self,
        req: &PluginInternalRequest,
    ) -> Option<http::Response<ResponseBody>> {
        self.internal_routes.get(req.path).map(|h| h.handle(req))
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

    /// Validate dependencies and return topological init order (Kahn's algorithm).
    /// Checks that all required deps exist while building the adjacency graph.
    fn resolve_init_order(&self) -> Result<Vec<usize>, PluginError> {
        let name_to_idx: HashMap<&str, usize> = self
            .plugins
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name(), i))
            .collect();

        let n = self.plugins.len();
        let mut in_degree = vec![0usize; n];
        let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];

        for (idx, plugin) in self.plugins.iter().enumerate() {
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
        fn shutdown(&self) {
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
        assert_eq!(health[0], ("healthy", PluginHealth::Ok));
        assert_eq!(health[1], ("degraded", PluginHealth::Degraded));
        assert_eq!(health[2], ("failed", PluginHealth::Failed));
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
            fn shutdown(&self) {
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
}
