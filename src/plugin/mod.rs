pub mod context;
pub mod cookies;
pub mod handler;
pub mod macros;
pub mod manager;
pub mod php;
pub mod wrappers;

pub use context::PluginContext;
pub use cookies::{CookieOptions, PluginCookies, PluginSetCookie, SameSite};
pub use handler::{
    PluginCompleteHandler, PluginCompleteView, PluginInternalHandler, PluginInternalRequest,
    PluginMetricsCollector, PluginRequestActions, PluginRequestHandler, PluginRequestView,
    PluginResponseActions, PluginResponseHandler, PluginResponseView,
};
pub use manager::PluginManager;
pub use php::{
    PhpArray, PhpArrayKey, PhpCallContext, PhpError, PhpObject, PhpParam, PhpType, PhpValue,
    PluginPhpFunction,
};

use std::any::Any;

/// Minimal plugin trait — 3 required methods, 4 optional.
pub trait Plugin: Send + Sync + Any {
    /// Unique plugin identifier (lowercase, alphanumeric + underscores).
    fn name(&self) -> &'static str;

    /// Plugin version (semver).
    fn version(&self) -> &'static str;

    /// Initialize: register handlers, services, config.
    /// Called after dependency resolution, in topological order.
    fn init(&mut self, ctx: &mut PluginContext) -> Result<(), PluginError>;

    /// Called after ALL plugins initialized and dispatcher frozen.
    fn on_ready(&self) {}

    /// Cleanup (called in reverse init order during shutdown).
    fn shutdown(&self) {}

    /// Plugin dependencies (other plugins / services).
    fn dependencies(&self) -> PluginDeps {
        PluginDeps::default()
    }

    /// Health check (called by /health endpoint).
    fn health(&self) -> PluginHealth {
        PluginHealth::Ok
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Dependency missing: {0}")]
    DependencyMissing(String),

    #[error("Service registration failed: {0}")]
    ServiceRegistration(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginHealth {
    Ok,
    Degraded,
    Failed,
}

impl PluginHealth {
    pub fn as_str(&self) -> &'static str {
        match self {
            PluginHealth::Ok => "ok",
            PluginHealth::Degraded => "degraded",
            PluginHealth::Failed => "failed",
        }
    }
}

#[derive(Default)]
pub struct PluginDeps {
    /// Required plugins (must be loaded).
    pub required: Vec<&'static str>,
    /// Optional plugins (enhance if available).
    pub optional: Vec<&'static str>,
    /// Required services (from other plugins).
    pub services: Vec<&'static str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_error_display() {
        let e = PluginError::Config("bad value".into());
        assert_eq!(e.to_string(), "Configuration error: bad value");

        let e = PluginError::DependencyMissing("auth".into());
        assert_eq!(e.to_string(), "Dependency missing: auth");

        let e = PluginError::ServiceRegistration("db pool".into());
        assert_eq!(e.to_string(), "Service registration failed: db pool");
    }

    #[test]
    fn test_plugin_health_as_str() {
        assert_eq!(PluginHealth::Ok.as_str(), "ok");
        assert_eq!(PluginHealth::Degraded.as_str(), "degraded");
        assert_eq!(PluginHealth::Failed.as_str(), "failed");
    }

    #[test]
    fn test_plugin_deps_default() {
        let deps = PluginDeps::default();
        assert!(deps.required.is_empty());
        assert!(deps.optional.is_empty());
        assert!(deps.services.is_empty());
    }
}
