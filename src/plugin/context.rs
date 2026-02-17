use std::any::Any;
use std::collections::HashMap;

use crate::events::EventDispatcher;

use super::handler::{
    PluginCompleteHandler, PluginInternalHandler, PluginMetricsCollector, PluginRequestHandler,
    PluginResponseHandler,
};
use super::php::{PhpParam, PhpType, PluginNativeFunction, PluginNativeFunctionDef};
use super::wrappers::{PluginCompleteWrapper, PluginRequestWrapper, PluginResponseWrapper};

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

    /// Plugin name.
    pub fn plugin_name(&self) -> &str {
        &self.plugin_name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventDispatcher;
    use crate::plugin::handler::PluginInternalRequest;

    fn make_context<'a>(
        dispatcher: &'a mut EventDispatcher,
        services: &'a mut HashMap<String, Box<dyn Any + Send + Sync>>,
        config_values: &'a mut HashMap<String, serde_json::Value>,
        metrics_collectors: &'a mut Vec<Box<dyn PluginMetricsCollector>>,
        internal_routes: &'a mut HashMap<String, Box<dyn PluginInternalHandler>>,
        native_php_functions: &'a mut Vec<PluginNativeFunctionDef>,
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

        let ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config,
            &mut metrics,
            &mut routes,
            &mut native_php,
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

        let ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config,
            &mut metrics,
            &mut routes,
            &mut native_php,
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

        {
            let mut ctx = make_context(
                &mut dispatcher,
                &mut services,
                &mut config,
                &mut metrics,
                &mut routes,
                &mut native_php,
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

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config,
            &mut metrics,
            &mut routes,
            &mut native_php,
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

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics,
            &mut routes,
            &mut native_php,
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

        let mut ctx = make_context(
            &mut dispatcher,
            &mut services,
            &mut config_values,
            &mut metrics,
            &mut routes,
            &mut native_php_functions,
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
}
