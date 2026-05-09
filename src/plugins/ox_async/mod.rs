pub mod functions;
pub mod synthetic;

use crate::bridge::call::NativeCall;
use crate::plugin::types::MagicMethod;
use crate::plugin::types::PhpType;
use crate::plugin::types::Visibility;
use crate::plugin::{PhpError, Plugin, PluginContext, PluginDeps, PluginError, PluginHealth};
use std::ffi::CString;

/// Read a private array property of `$this` and copy it into the return
/// value. If the property is unset, null, or `$this` is not available,
/// return an empty array. Used by the `AggregateAsyncException` accessor
/// methods (`getErrors`, `getErrorMap`, `getPromiseIds`); the C bridge
/// populates the underlying properties at construction time.
fn return_property_array(call: &mut NativeCall, name: &str) -> Result<(), PhpError> {
    use crate::bridge::ffi;

    let this = call.this_ptr();
    if this.is_null() {
        call.ret_array(0, |_| {});
        return Ok(());
    }

    let cname = match CString::new(name) {
        Ok(c) => c,
        Err(_) => {
            call.ret_array(0, |_| {});
            return Ok(());
        }
    };

    let prop = unsafe { ffi::oxphp_object_read_property(this, cname.as_ptr()) };
    if prop.is_null() || unsafe { ffi::oxphp_zval_is_null_or_unset(prop) } != 0 {
        call.ret_array(0, |_| {});
        return Ok(());
    }

    unsafe { ffi::oxphp_zval_copy_to_retval(prop, call.retval_ptr()) };
    Ok(())
}

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
        // Publish synthetic-promise C-ABI shims to the bridge. Must run
        // on the main thread before PHP workers spawn; `init` is called
        // exactly once in that slot.
        synthetic::register_with_bridge();

        // Determine enabled state: ASYNC_WORKERS=0 (or absent) means disabled.
        let workers: u32 = ctx
            .config("WORKERS")
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        self.enabled = workers > 0;

        // ── Exception classes (always registered) ─────────────────────────

        // OxPHP\Async\AsyncException extends \Exception
        ctx.register_class("OxPHP\\Async\\AsyncException")
            .extends("Exception")
            .build()?;

        // OxPHP\Async\TimeoutException extends OxPHP\Async\AsyncException
        // Carries optional partial-errors / pending-ids context (populated only
        // by oxphp_async_await_any timeout path; empty for all other timeouts).
        ctx.register_class("OxPHP\\Async\\TimeoutException")
            .extends("OxPHP\\Async\\AsyncException")
            .property("__partialErrors", PhpType::Array, Visibility::Private)
            .property("__pendingPromiseIds", PhpType::Array, Visibility::Private)
            .method("getPartialErrors")
            .returns(PhpType::Array)
            .handler(|call| return_property_array(call, "__partialErrors"))
            .method("getPendingPromiseIds")
            .returns(PhpType::Array)
            .handler(|call| return_property_array(call, "__pendingPromiseIds"))
            .build()?;

        // OxPHP\Async\AggregateAsyncException extends OxPHP\Async\AsyncException
        // Carries a list of errors (one per rejected promise) when every
        // promise passed to oxphp_async_await_any() rejected before any could fulfill.
        ctx.register_class("OxPHP\\Async\\AggregateAsyncException")
            .extends("OxPHP\\Async\\AsyncException")
            .property("__errors", PhpType::Array, Visibility::Private)
            .property("__errorMap", PhpType::Array, Visibility::Private)
            .property("__promiseIds", PhpType::Array, Visibility::Private)
            .method("getErrors")
            .returns(PhpType::Array)
            .handler(|call| return_property_array(call, "__errors"))
            .method("getErrorMap")
            .returns(PhpType::Array)
            .handler(|call| return_property_array(call, "__errorMap"))
            .method("getPromiseIds")
            .returns(PhpType::Array)
            .handler(|call| return_property_array(call, "__promiseIds"))
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
            tracing::info!(plugin = "async", workers, "Async plugin initialized");
        } else {
            tracing::info!(
                plugin = "async",
                "Async plugin disabled (ASYNC_WORKERS=0 or not set)"
            );
        }

        Ok(())
    }

    fn shutdown(&mut self) {
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
            "async".into(),
            "__oxp_async_".into(),
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
            "async".into(),
            "__oxp_async_".into(),
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

        let mut plugin = AsyncPlugin::new();
        plugin.init(&mut ctx).unwrap();
        drop(ctx);

        // Should register 5 classes: AsyncException, TimeoutException,
        // AggregateAsyncException, BorrowException, BorrowedProxy
        assert_eq!(php_classes.len(), 5);

        let fqns: Vec<&str> = php_classes.iter().map(|c| c.fqn.as_str()).collect();
        assert!(fqns.contains(&"OxPHP\\Async\\AsyncException"));
        assert!(fqns.contains(&"OxPHP\\Async\\TimeoutException"));
        assert!(fqns.contains(&"OxPHP\\Async\\AggregateAsyncException"));
        assert!(fqns.contains(&"OxPHP\\Async\\BorrowException"));
        assert!(fqns.contains(&"OxPHP\\Async\\BorrowedProxy"));
    }

    #[test]
    fn aggregate_async_exception_methods_registered() {
        std::env::remove_var("ASYNC_WORKERS");

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
            "async".into(),
            "__oxp_async_".into(),
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

        let mut plugin = AsyncPlugin::new();
        plugin.init(&mut ctx).unwrap();
        drop(ctx);

        let agg = php_classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Async\\AggregateAsyncException")
            .expect("AggregateAsyncException must be registered");

        // Properties: __errors, __errorMap, __promiseIds (all private arrays).
        let prop_names: Vec<&str> = agg.properties.iter().map(|p| p.name.as_str()).collect();
        assert!(prop_names.contains(&"__errors"));
        assert!(prop_names.contains(&"__errorMap"));
        assert!(prop_names.contains(&"__promiseIds"));
        for prop in &agg.properties {
            assert_eq!(prop.visibility, Visibility::Private);
            assert_eq!(prop.php_type, PhpType::Array);
        }

        // Methods: getErrors, getErrorMap, getPromiseIds (each with a handler).
        let method_names: Vec<&str> = agg.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(method_names.contains(&"getErrors"));
        assert!(method_names.contains(&"getErrorMap"));
        assert!(method_names.contains(&"getPromiseIds"));
        for m in &agg.methods {
            assert!(m.handler.is_some(), "method {} must have a handler", m.name);
            assert_eq!(m.return_type, Some(PhpType::Array));
        }
    }

    #[test]
    fn timeout_exception_has_partial_errors_methods() {
        std::env::remove_var("ASYNC_WORKERS");

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
            "async".into(),
            "__oxp_async_".into(),
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

        let mut plugin = AsyncPlugin::new();
        plugin.init(&mut ctx).unwrap();
        drop(ctx);

        let to = php_classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Async\\TimeoutException")
            .expect("TimeoutException must be registered");

        // Properties: __partialErrors, __pendingPromiseIds (private arrays).
        let prop_names: Vec<&str> = to.properties.iter().map(|p| p.name.as_str()).collect();
        assert!(prop_names.contains(&"__partialErrors"));
        assert!(prop_names.contains(&"__pendingPromiseIds"));
        for prop in &to.properties {
            assert_eq!(prop.visibility, Visibility::Private);
            assert_eq!(prop.php_type, PhpType::Array);
        }

        // Methods: getPartialErrors, getPendingPromiseIds (each with a handler).
        let method_names: Vec<&str> = to.methods.iter().map(|m| m.name.as_str()).collect();
        assert!(method_names.contains(&"getPartialErrors"));
        assert!(method_names.contains(&"getPendingPromiseIds"));
        for m in &to.methods {
            assert!(m.handler.is_some(), "method {} must have a handler", m.name);
            assert_eq!(m.return_type, Some(PhpType::Array));
        }
    }

    #[test]
    fn test_borrowed_proxy_json_serialize_return_type() {
        use crate::plugin::types::{PhpType, BRIDGE_RT_MIXED};

        std::env::remove_var("ASYNC_WORKERS");

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
            "async".into(),
            "__oxp_async_".into(),
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

        let mut plugin = AsyncPlugin::new();
        plugin.init(&mut ctx).unwrap();
        drop(ctx);

        // Find BorrowedProxy class
        let proxy = php_classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Async\\BorrowedProxy")
            .expect("BorrowedProxy class should be registered");

        // Must implement JsonSerializable
        assert!(
            proxy.interfaces.contains(&"JsonSerializable".to_string()),
            "BorrowedProxy must implement JsonSerializable"
        );

        // jsonSerialize method must have return type Mixed
        let method = proxy
            .methods
            .iter()
            .find(|m| m.name == "jsonSerialize")
            .expect("jsonSerialize method should exist");
        assert_eq!(
            method.return_type,
            Some(PhpType::Mixed),
            "jsonSerialize must declare return type Mixed for PHP 8.x compatibility"
        );

        // Verify bridge tag produces correct constant
        let (tag, nullable) = method.return_type.as_ref().unwrap().to_bridge_tag();
        assert_eq!(tag, BRIDGE_RT_MIXED);
        assert!(!nullable);
    }

    #[test]
    fn test_borrowed_proxy_all_methods_have_handlers() {
        std::env::remove_var("ASYNC_WORKERS");

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
            "async".into(),
            "__oxp_async_".into(),
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

        let mut plugin = AsyncPlugin::new();
        plugin.init(&mut ctx).unwrap();
        drop(ctx);

        let proxy = php_classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Async\\BorrowedProxy")
            .expect("BorrowedProxy class should be registered");

        // jsonSerialize is the only explicit method (magic methods are separate)
        assert_eq!(proxy.methods.len(), 1);
        assert!(
            proxy.methods[0].handler.is_some(),
            "jsonSerialize must have a handler"
        );

        // All magic method slots that BorrowedProxy uses should have handlers
        use crate::plugin::types::MagicMethod;
        for magic in [
            MagicMethod::Get,
            MagicMethod::Set,
            MagicMethod::Call,
            MagicMethod::ToString,
            MagicMethod::DebugInfo,
        ] {
            assert!(
                proxy.magic_handlers[magic.index()].is_some(),
                "BorrowedProxy must have handler for {}",
                magic.php_name()
            );
        }
    }

    #[test]
    fn test_shutdown_noop_when_disabled() {
        let mut plugin = AsyncPlugin::new();
        plugin.shutdown(); // should not panic
    }
}
