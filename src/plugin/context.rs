use std::any::Any;
use std::collections::HashMap;

use crate::decorator::Decorator;
use crate::events::EventDispatcher;

use super::builders::attribute::AttributeBuilder;
use super::builders::class::ClassBuilder;
use super::builders::definitions::*;
use super::builders::enum_::EnumBuilder;
use super::builders::function::FunctionBuilder;
use super::builders::interface::InterfaceBuilder;
use super::handler::{
    PluginCompleteHandler, PluginInternalHandler, PluginMetricsCollector, PluginRequestHandler,
    PluginResponseHandler,
};
use super::php::{PhpParam, PhpType, PluginNativeFunction, PluginNativeFunctionDef};
use super::wrappers::{PluginCompleteWrapper, PluginRequestWrapper, PluginResponseWrapper};

/// Definition collected during plugin init for a Rust-native decorator.
pub struct PluginDecoratorDef {
    pub decorator: Box<dyn Decorator>,
}

/// Restricted API passed to `Plugin::init()`.
/// Plugins can register handlers, config, services, and PHP functions through this.
pub struct PluginContext<'a> {
    plugin_name: String,
    cookie_prefix: String,
    dispatcher: &'a mut EventDispatcher,
    services: &'a mut HashMap<String, Box<dyn Any + Send + Sync>>,
    config_values: &'a mut HashMap<String, serde_json::Value>,
    metrics_collectors: &'a mut Vec<Box<dyn PluginMetricsCollector>>,
    internal_routes: &'a mut HashMap<String, Box<dyn PluginInternalHandler>>,
    native_php_functions: &'a mut Vec<PluginNativeFunctionDef>,
    decorators: &'a mut Vec<PluginDecoratorDef>,
    php_classes: &'a mut Vec<PhpClassDef>,
    php_interfaces: &'a mut Vec<PhpInterfaceDef>,
    php_enums: &'a mut Vec<PhpEnumDef>,
    php_attributes: &'a mut Vec<PhpAttributeDef>,
    php_functions: &'a mut Vec<PhpFunctionDef>,
}

impl<'a> PluginContext<'a> {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        plugin_name: String,
        cookie_prefix: String,
        dispatcher: &'a mut EventDispatcher,
        services: &'a mut HashMap<String, Box<dyn Any + Send + Sync>>,
        config_values: &'a mut HashMap<String, serde_json::Value>,
        metrics_collectors: &'a mut Vec<Box<dyn PluginMetricsCollector>>,
        internal_routes: &'a mut HashMap<String, Box<dyn PluginInternalHandler>>,
        native_php_functions: &'a mut Vec<PluginNativeFunctionDef>,
        decorators: &'a mut Vec<PluginDecoratorDef>,
        php_classes: &'a mut Vec<PhpClassDef>,
        php_interfaces: &'a mut Vec<PhpInterfaceDef>,
        php_enums: &'a mut Vec<PhpEnumDef>,
        php_attributes: &'a mut Vec<PhpAttributeDef>,
        php_functions: &'a mut Vec<PhpFunctionDef>,
    ) -> Self {
        Self {
            plugin_name,
            cookie_prefix,
            dispatcher,
            services,
            config_values,
            metrics_collectors,
            internal_routes,
            native_php_functions,
            decorators,
            php_classes,
            php_interfaces,
            php_enums,
            php_attributes,
            php_functions,
        }
    }

    /// Register a request handler (restricted: read-only request + metadata + early_response).
    pub fn on_request(&mut self, handler: impl PluginRequestHandler + 'static) {
        let wrapper = PluginRequestWrapper {
            handler,
            plugin_name: self.plugin_name.clone(),
            cookie_prefix: self.cookie_prefix.clone(),
        };
        self.dispatcher.on(wrapper);
    }

    /// Register a response handler (restricted: read-only response + add headers/cookies).
    pub fn on_response(&mut self, handler: impl PluginResponseHandler + 'static) {
        let wrapper = PluginResponseWrapper {
            handler,
            plugin_name: self.plugin_name.clone(),
            cookie_prefix: self.cookie_prefix.clone(),
        };
        self.dispatcher.on(wrapper);
    }

    /// Register a completion handler (read-only, for logging/analytics).
    pub fn on_complete(&mut self, handler: impl PluginCompleteHandler + 'static) {
        let wrapper = PluginCompleteWrapper {
            handler,
            plugin_name: self.plugin_name.clone(),
        };
        self.dispatcher.on(wrapper);
    }

    /// Read config: checks `{PLUGIN_NAME}_{KEY}` env var first, then `{KEY}`.
    pub fn config(&self, key: &str) -> Option<String> {
        let prefixed = format!("{}_{}", self.plugin_name.to_uppercase(), key);
        std::env::var(&prefixed)
            .or_else(|_| std::env::var(key))
            .ok()
    }

    /// Register a typed service (shared with other plugins).
    pub fn register_service(&mut self, name: &str, service: Box<dyn Any + Send + Sync>) {
        self.services.insert(name.to_string(), service);
    }

    /// Get a service by name and type.
    pub fn service<T: Any>(&self, name: &str) -> Option<&T> {
        self.services.get(name)?.downcast_ref::<T>()
    }

    /// Expose a config value in the /config endpoint under `plugins.{name}.{key}`.
    pub fn expose_config(&mut self, key: &str, value: impl Into<serde_json::Value>) {
        self.config_values.insert(key.to_string(), value.into());
    }

    /// Register a Prometheus metrics collector.
    pub fn register_metrics(&mut self, collector: impl PluginMetricsCollector + 'static) {
        self.metrics_collectors.push(Box::new(collector));
    }

    /// Register a route on the internal server.
    /// Path must start with `/__{plugin_name}/` (enforced).
    pub fn internal_route(&mut self, path: &str, handler: impl PluginInternalHandler + 'static) {
        let prefix = format!("/__{}/", self.plugin_name);
        if !path.starts_with(&prefix) {
            tracing::warn!(
                plugin = %self.plugin_name,
                path,
                expected_prefix = %prefix,
                "Internal route rejected: path must start with plugin prefix"
            );
            return;
        }
        self.internal_routes
            .insert(path.to_string(), Box::new(handler));
    }

    /// Register a native PHP function (zero-serialization bridge).
    /// Function name is auto-prefixed: `oxphp_{plugin_name}_{name}`.
    pub fn register_function(
        &mut self,
        name: &str,
        params: Vec<PhpParam>,
        return_type: PhpType,
        handler: impl PluginNativeFunction + 'static,
    ) {
        let full_name = format!("oxphp_{}_{}", self.plugin_name, name);
        self.native_php_functions.push(PluginNativeFunctionDef {
            name: full_name,
            plugin_name: self.plugin_name.clone(),
            params,
            return_type,
            handler: Box::new(handler),
        });
    }

    /// Register a native PHP function with an exact name (no auto-prefix).
    /// Use for functions that should appear as top-level `oxphp_*` builtins.
    pub fn register_function_as(
        &mut self,
        full_name: &str,
        params: Vec<PhpParam>,
        return_type: PhpType,
        handler: impl PluginNativeFunction + 'static,
    ) {
        self.native_php_functions.push(PluginNativeFunctionDef {
            name: full_name.to_string(),
            plugin_name: self.plugin_name.clone(),
            params,
            return_type,
            handler: Box::new(handler),
        });
    }

    /// Register a Rust-native decorator.
    /// The decorator's `attribute_name()` is the fully qualified PHP attribute class name.
    pub fn register_decorator(&mut self, decorator: impl Decorator + 'static) {
        self.decorators.push(PluginDecoratorDef {
            decorator: Box::new(decorator),
        });
    }

    /// Register a PHP class definition.
    pub fn register_class(&mut self, fqn: &str) -> ClassBuilder<'_> {
        ClassBuilder::new(fqn, &self.plugin_name, &mut self.php_classes)
    }

    /// Register a PHP interface definition.
    pub fn register_interface(&mut self, fqn: &str) -> InterfaceBuilder<'_> {
        InterfaceBuilder::new(fqn, &self.plugin_name, &mut self.php_interfaces)
    }

    /// Register a PHP enum definition.
    pub fn register_enum(&mut self, fqn: &str) -> EnumBuilder<'_> {
        EnumBuilder::new(fqn, &self.plugin_name, &mut self.php_enums)
    }

    /// Register a PHP attribute definition.
    pub fn register_attribute(&mut self, fqn: &str) -> AttributeBuilder<'_> {
        AttributeBuilder::new(fqn, &self.plugin_name, &mut self.php_attributes)
    }

    /// Register a free PHP function definition.
    pub fn function(&mut self, fqn: &str) -> FunctionBuilder<'_> {
        FunctionBuilder::new(fqn, &self.plugin_name, &mut self.php_functions)
    }

    /// Plugin name.
    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decorator::{
        AttributeTargets, DecoratorAction, DecoratorCallContext, DecoratorCallResult,
    };
    use crate::events::EventDispatcher;
    use crate::plugin::handler::PluginInternalRequest;

    fn make_context<'a>(
        dispatcher: &'a mut EventDispatcher,
        services: &'a mut HashMap<String, Box<dyn Any + Send + Sync>>,
        config_values: &'a mut HashMap<String, serde_json::Value>,
        metrics_collectors: &'a mut Vec<Box<dyn PluginMetricsCollector>>,
        internal_routes: &'a mut HashMap<String, Box<dyn PluginInternalHandler>>,
        native_php_functions: &'a mut Vec<PluginNativeFunctionDef>,
        decorators: &'a mut Vec<PluginDecoratorDef>,
        php_classes: &'a mut Vec<PhpClassDef>,
        php_interfaces: &'a mut Vec<PhpInterfaceDef>,
        php_enums: &'a mut Vec<PhpEnumDef>,
        php_attributes: &'a mut Vec<PhpAttributeDef>,
        php_functions: &'a mut Vec<PhpFunctionDef>,
    ) -> PluginContext<'a> {
        PluginContext::new(
            "test_plugin".into(),
            "__oxp_test_plugin_".into(),
            dispatcher,
            services,
            config_values,
            metrics_collectors,
            internal_routes,
            native_php_functions,
            decorators,
            php_classes,
            php_interfaces,
            php_enums,
            php_attributes,
            php_functions,
        )
    }

    #[test]
    fn test_config_lookup_with_prefix() {
        std::env::set_var("TEST_PLUGIN_API_KEY", "secret");
        std::env::set_var("_OXPHP_TEST_SHARED_KEY", "shared");

        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes = HashMap::new();
        let mut native_php = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config,
            &mut metrics,
            &mut routes,
            &mut native_php,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        assert_eq!(ctx.config("API_KEY"), Some("secret".to_string()));
        assert_eq!(
            ctx.config("_OXPHP_TEST_SHARED_KEY"),
            Some("shared".to_string())
        );
        assert_eq!(ctx.config("MISSING"), None);

        std::env::remove_var("TEST_PLUGIN_API_KEY");
        std::env::remove_var("_OXPHP_TEST_SHARED_KEY");
    }

    #[test]
    fn test_plugin_name() {
        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes = HashMap::new();
        let mut native_php = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config,
            &mut metrics,
            &mut routes,
            &mut native_php,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        assert_eq!(ctx.plugin_name(), "test_plugin");
    }

    #[test]
    fn test_internal_route_prefix_enforcement() {
        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes: HashMap<String, Box<dyn PluginInternalHandler>> = HashMap::new();
        let mut native_php = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        {
            let mut ctx = make_context(
                &mut dispatcher,
                &mut services,
                &mut config,
                &mut metrics,
                &mut routes,
                &mut native_php,
                &mut decorators,
                &mut php_classes,
                &mut php_interfaces,
                &mut php_enums,
                &mut php_attributes,
                &mut php_functions,
            );

            // Valid path
            ctx.internal_route("/__test_plugin/status", |_req: &PluginInternalRequest| {
                http::Response::builder()
                    .status(200)
                    .body(crate::types::full_body(bytes::Bytes::from_static(b"ok")))
                    .unwrap()
            });

            // Invalid path (wrong prefix)
            ctx.internal_route("/admin/status", |_req: &PluginInternalRequest| {
                http::Response::builder()
                    .status(200)
                    .body(crate::types::full_body(bytes::Bytes::from_static(b"ok")))
                    .unwrap()
            });
        }

        assert!(routes.contains_key("/__test_plugin/status"));
        assert!(!routes.contains_key("/admin/status"));
    }

    #[test]
    fn test_service_registration() {
        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes = HashMap::new();
        let mut native_php = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config,
            &mut metrics,
            &mut routes,
            &mut native_php,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        ctx.register_service("my_pool", Box::new(42u32));
        assert_eq!(ctx.service::<u32>("my_pool"), Some(&42u32));
        assert_eq!(ctx.service::<String>("my_pool"), None); // wrong type
        assert_eq!(ctx.service::<u32>("other"), None); // wrong name
    }

    #[test]
    fn test_expose_config() {
        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes = HashMap::new();
        let mut native_php = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics,
            &mut routes,
            &mut native_php,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        ctx.expose_config("verbose", serde_json::json!(true));
        assert_eq!(config_values.get("verbose"), Some(&serde_json::json!(true)));
    }

    #[test]
    fn test_register_function() {
        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes = HashMap::new();
        let mut native_php_functions = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics,
            &mut routes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        ctx.register_function(
            "echo_upper",
            vec![PhpParam::required("input", PhpType::String)],
            PhpType::String,
            |_call: &mut crate::bridge::call::NativeCall| Ok(()),
        );

        drop(ctx);

        assert_eq!(native_php_functions.len(), 1);
        assert_eq!(native_php_functions[0].name, "oxphp_test_plugin_echo_upper");
    }

    #[test]
    fn test_register_decorator() {
        struct TestDecorator;
        impl Decorator for TestDecorator {
            fn attribute_name(&self) -> &str {
                "App\\TestDec"
            }
            fn targets(&self) -> AttributeTargets {
                AttributeTargets::ALL
            }
            fn on_begin(&self, _: &DecoratorCallContext) -> DecoratorAction {
                DecoratorAction::Continue
            }
            fn on_end(&self, _: &DecoratorCallContext, _: &DecoratorCallResult) {}
        }

        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes = HashMap::new();
        let mut native_php_functions = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics,
            &mut routes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        ctx.register_decorator(TestDecorator);
        drop(ctx);

        assert_eq!(decorators.len(), 1);
        assert_eq!(decorators[0].decorator.attribute_name(), "App\\TestDec");
    }

    #[test]
    fn test_register_function_as() {
        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes = HashMap::new();
        let mut native_php_functions = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics,
            &mut routes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        ctx.register_function_as(
            "oxphp_trace_start",
            vec![PhpParam::required("name", PhpType::String)],
            PhpType::Int,
            |_call: &mut crate::bridge::call::NativeCall| Ok(()),
        );

        drop(ctx);

        assert_eq!(native_php_functions.len(), 1);
        // register_function_as uses exact name — no prefix added
        assert_eq!(native_php_functions[0].name, "oxphp_trace_start");
        assert_eq!(native_php_functions[0].plugin_name, "test_plugin");
    }

    #[test]
    fn test_register_class() {
        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes = HashMap::new();
        let mut native_php_functions = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics,
            &mut routes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        let _ = ctx.register_class("App\\MyClass").build();
        drop(ctx);

        assert_eq!(php_classes.len(), 1);
        assert_eq!(php_classes[0].fqn, "App\\MyClass");
        assert_eq!(php_classes[0].plugin_name, "test_plugin");
    }

    #[test]
    fn test_register_interface() {
        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes = HashMap::new();
        let mut native_php_functions = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics,
            &mut routes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        let _ = ctx.register_interface("App\\Countable").build();
        drop(ctx);

        assert_eq!(php_interfaces.len(), 1);
        assert_eq!(php_interfaces[0].fqn, "App\\Countable");
        assert_eq!(php_interfaces[0].plugin_name, "test_plugin");
    }

    #[test]
    fn test_register_enum() {
        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes = HashMap::new();
        let mut native_php_functions = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics,
            &mut routes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        let _ = ctx.register_enum("App\\Status").build();
        drop(ctx);

        assert_eq!(php_enums.len(), 1);
        assert_eq!(php_enums[0].fqn, "App\\Status");
        assert_eq!(php_enums[0].plugin_name, "test_plugin");
    }

    #[test]
    fn test_register_attribute() {
        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes = HashMap::new();
        let mut native_php_functions = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics,
            &mut routes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        let _ = ctx.register_attribute("App\\Route").build();
        drop(ctx);

        assert_eq!(php_attributes.len(), 1);
        assert_eq!(php_attributes[0].fqn, "App\\Route");
        assert_eq!(php_attributes[0].plugin_name, "test_plugin");
    }

    #[test]
    fn test_function_builder() {
        let mut dispatcher = EventDispatcher::new();
        let mut services = HashMap::new();
        let mut config_values = HashMap::new();
        let mut metrics = Vec::new();
        let mut routes = HashMap::new();
        let mut native_php_functions = Vec::new();
        let mut decorators = Vec::new();
        let mut php_classes = Vec::new();
        let mut php_interfaces = Vec::new();
        let mut php_enums = Vec::new();
        let mut php_attributes = Vec::new();
        let mut php_functions = Vec::new();

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics,
            &mut routes,
            &mut native_php_functions,
            &mut decorators,
            &mut php_classes,
            &mut php_interfaces,
            &mut php_enums,
            &mut php_attributes,
            &mut php_functions,
        );

        let _ = ctx
            .function("oxphp_test_plugin_hello")
            .handler(|_call| Ok(()));
        drop(ctx);

        assert_eq!(php_functions.len(), 1);
        assert_eq!(php_functions[0].fqn, "oxphp_test_plugin_hello");
        assert_eq!(php_functions[0].plugin_name, "test_plugin");
    }
}
