pub mod functions;

use crate::plugin::types::MagicMethod;
use crate::plugin::types::PhpType;
use crate::plugin::{PhpError, Plugin, PluginContext, PluginDeps, PluginError, PluginHealth};

// ─── Borrow error helper ──────────────────────────────────────────────────────

/// Builds the standard `BorrowException` error thrown by all BorrowedProxy
/// magic methods and `jsonSerialize`.
fn borrow_err(method: &str) -> PhpError {
    PhpError::Exception {
        class: "OxPHP\\Async\\BorrowException".to_string(),
        message: format!(
            "Cannot access borrowed object via {method} \u{2014} awaiting async promise"
        ),
        code: 0,
    }
}

// ─── AsyncPlugin ─────────────────────────────────────────────────────────────

/// Async plugin — provides `OxPHP\Async\*` PHP classes and helper functions.
///
/// Feature-gated behind `plugin-async`. Reads `ASYNC_WORKERS` to decide
/// whether async execution is enabled. Exception classes and BorrowedProxy
/// are always registered so PHP code that references them does not fatal even
/// when the plugin is disabled.
pub struct AsyncPlugin {
    enabled: bool,
}

impl Default for AsyncPlugin {
    fn default() -> Self {
        Self::new()
    }
}

impl AsyncPlugin {
    pub fn new() -> Self {
        Self { enabled: false }
    }
}

impl Plugin for AsyncPlugin {
    fn name(&self) -> &'static str {
        "async"
    }

    fn version(&self) -> &'static str {
        env!("CARGO_PKG_VERSION")
    }

    fn dependencies(&self) -> PluginDeps {
        PluginDeps::default()
    }

    fn init(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError> {
        // Determine enabled state: ASYNC_WORKERS=0 (or absent) means disabled.
        let workers: u32 = ctx
            .config("WORKERS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        self.enabled = workers > 0;

        // ── Exception classes (always registered) ─────────────────────────

        // OxPHP\Async\Exception extends \Exception
        ctx.register_class("OxPHP\\Async\\Exception")
            .extends("Exception")
            .build()?;

        // OxPHP\Async\TimeoutException extends OxPHP\Async\Exception
        ctx.register_class("OxPHP\\Async\\TimeoutException")
            .extends("OxPHP\\Async\\Exception")
            .build()?;

        // OxPHP\Async\BorrowException extends \Exception
        ctx.register_class("OxPHP\\Async\\BorrowException")
            .extends("Exception")
            .build()?;

        // ── BorrowedProxy (always registered) ─────────────────────────────
        //
        // All access methods throw BorrowException to prevent PHP code from
        // reading properties of a not-yet-resolved async promise object.

        ctx.register_class("OxPHP\\Async\\BorrowedProxy")
            .implements("JsonSerializable")
            // __get
            .magic(MagicMethod::Get)
            .handler(|_call| Err(borrow_err("__get")))
            // __set
            .magic(MagicMethod::Set)
            .handler(|_call| Err(borrow_err("__set")))
            // __call
            .magic(MagicMethod::Call)
            .handler(|_call| Err(borrow_err("__call")))
            // __isset
            .magic(MagicMethod::Isset)
            .handler(|_call| Err(borrow_err("__isset")))
            // __unset
            .magic(MagicMethod::Unset)
            .handler(|_call| Err(borrow_err("__unset")))
            // __toString
            .magic(MagicMethod::ToString)
            .handler(|_call| Err(borrow_err("__toString")))
            // __debugInfo
            .magic(MagicMethod::DebugInfo)
            .handler(|_call| Err(borrow_err("__debugInfo")))
            // jsonSerialize (regular method required by JsonSerializable)
            .method("jsonSerialize")
            .returns(PhpType::Mixed)
            .handler(|_call| Err(borrow_err("jsonSerialize")))
            .build()?;

        // ── Functions ─────────────────────────────────────────────────────
        functions::register_functions(ctx, self.enabled)?;

        // ── Config exposure ───────────────────────────────────────────────
        ctx.expose_config("enabled", self.enabled);
        if self.enabled {
            ctx.expose_config("workers", workers);
            tracing::info!(
                plugin = "async",
                workers,
                "Async plugin initialized"
            );
        } else {
            tracing::info!(
                plugin = "async",
                "Async plugin disabled (ASYNC_WORKERS=0 or not set)"
            );
        }

        Ok(())
    }

    fn shutdown(&self) {
        if self.enabled {
            tracing::info!(plugin = "async", "Async plugin shutdown complete");
        }
    }

    fn health(&self) -> PluginHealth {
        PluginHealth::Ok
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventDispatcher;
    use crate::plugin::context::PluginDecoratorDef;
    use crate::plugin::handler::{PluginInternalHandler, PluginMetricsCollector};
    use crate::plugin::php::PluginNativeFunctionDef;
    use std::collections::HashMap;

    fn init_async_plugin(plugin: &mut AsyncPlugin) -> HashMap<String, serde_json::Value> {
        let mut dispatcher = EventDispatcher::new();
        let mut services: HashMap<String, Box<dyn std::any::Any + Send + Sync>> = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics_collectors: Vec<Box<dyn PluginMetricsCollector>> = Vec::new();
        let mut internal_routes: HashMap<String, Box<dyn PluginInternalHandler>> = HashMap::new();
        let mut native_php_functions: Vec<PluginNativeFunctionDef> = Vec::new();
        let mut decorators: Vec<PluginDecoratorDef> = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = PluginContext::new(
            "async".into(),
            "__oxp_async_".into(),
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics_collectors,
            &mut internal_routes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );
        plugin.init(&mut ctx).unwrap();
        drop(ctx);
        config_values
    }

    #[test]
    fn test_plugin_name_and_version() {
        let plugin = AsyncPlugin::new();
        assert_eq!(plugin.name(), "async");
        assert_eq!(plugin.version(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn test_borrow_err_format() {
        let err = borrow_err("__get");
        match err {
            PhpError::Exception {
                ref class,
                ref message,
                code,
            } => {
                assert_eq!(class, "OxPHP\\Async\\BorrowException");
                assert!(
                    message.contains("__get"),
                    "message should mention the method"
                );
                assert!(
                    message.contains("awaiting async promise"),
                    "message should mention async promise"
                );
                assert_eq!(code, 0);
            }
            _ => panic!("Expected PhpError::Exception"),
        }
    }

    #[test]
    fn test_plugin_disabled_by_default() {
        std::env::remove_var("ASYNC_WORKERS");
        let mut plugin = AsyncPlugin::new();
        let config = init_async_plugin(&mut plugin);

        assert!(!plugin.enabled);
        assert_eq!(config.get("enabled"), Some(&serde_json::json!(false)));
        assert_eq!(plugin.health(), PluginHealth::Ok);
    }

    #[test]
    fn test_plugin_enabled_via_env() {
        std::env::set_var("ASYNC_WORKERS", "4");
        let mut plugin = AsyncPlugin::new();
        let config = init_async_plugin(&mut plugin);

        assert!(plugin.enabled);
        assert_eq!(config.get("enabled"), Some(&serde_json::json!(true)));
        assert_eq!(config.get("workers"), Some(&serde_json::json!(4u64)));
        std::env::remove_var("ASYNC_WORKERS");
    }

    #[test]
    fn test_plugin_default_trait() {
        let plugin = AsyncPlugin::default();
        assert_eq!(plugin.name(), "async");
        assert!(!plugin.enabled);
    }

    #[test]
    fn test_exception_classes_registered() {
        std::env::remove_var("ASYNC_WORKERS");

        let mut dispatcher = EventDispatcher::new();
        let mut services: HashMap<String, Box<dyn std::any::Any + Send + Sync>> = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics_collectors: Vec<Box<dyn PluginMetricsCollector>> = Vec::new();
        let mut internal_routes: HashMap<String, Box<dyn PluginInternalHandler>> = HashMap::new();
        let mut native_php_functions: Vec<PluginNativeFunctionDef> = Vec::new();
        let mut decorators: Vec<PluginDecoratorDef> = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = PluginContext::new(
            "async".into(),
            "__oxp_async_".into(),
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics_collectors,
            &mut internal_routes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        let mut plugin = AsyncPlugin::new();
        plugin.init(&mut ctx).unwrap();
        drop(ctx);

        // Should register 4 classes: Exception, TimeoutException, BorrowException, BorrowedProxy
        assert_eq!(php_classes.len(), 4);

        let fqns: Vec<&str> = php_classes.iter().map(|c| c.fqn.as_str()).collect();
        assert!(fqns.contains(&"OxPHP\\Async\\Exception"));
        assert!(fqns.contains(&"OxPHP\\Async\\TimeoutException"));
        assert!(fqns.contains(&"OxPHP\\Async\\BorrowException"));
        assert!(fqns.contains(&"OxPHP\\Async\\BorrowedProxy"));
    }

    #[test]
    fn test_shutdown_noop_when_disabled() {
        let plugin = AsyncPlugin::new();
        plugin.shutdown(); // should not panic
    }
}
