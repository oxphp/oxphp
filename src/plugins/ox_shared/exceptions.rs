//! Register all Shared\* exception classes at plugin init.
//!
//! Spec: .internal/technical-docs/en/features/shared/05-exceptions.md
//!
//! Hierarchy (10 classes: 1 base + 9 subclasses):
//!
//!   \Exception
//!     └── OxPHP\Shared\Exception            (1)
//!          ├── StaleHandleException          (2)
//!          ├── TypeException                 (3)
//!          │    └── CycleException           (4)  Map::set would form a cycle
//!          ├── CapacityException             (5)
//!          ├── ClosedException               (6)
//!          ├── PoisonedException             (7)
//!          ├── TimeoutException              (8)
//!          │    └── DeadlockException        (9)
//!          └── UninitializedException        (10) access before first set()

use crate::plugin::{PluginContext, PluginError};

pub fn register_all(ctx: &mut PluginContext) -> Result<(), PluginError> {
    // Base.
    ctx.register_class("OxPHP\\Shared\\Exception")
        .extends("Exception")
        .build()?;

    // Direct subclasses.
    for child in [
        "OxPHP\\Shared\\StaleHandleException",
        "OxPHP\\Shared\\TypeException",
        "OxPHP\\Shared\\CapacityException",
        "OxPHP\\Shared\\ClosedException",
        "OxPHP\\Shared\\PoisonedException",
        "OxPHP\\Shared\\TimeoutException",
        "OxPHP\\Shared\\UninitializedException",
    ] {
        ctx.register_class(child)
            .extends("OxPHP\\Shared\\Exception")
            .build()?;
    }

    // CycleException extends TypeException (review decision v3 — a cycle
    // is a value-shape violation at the storage slot).
    ctx.register_class("OxPHP\\Shared\\CycleException")
        .extends("OxPHP\\Shared\\TypeException")
        .build()?;

    // DeadlockException extends TimeoutException (so catch(TimeoutException)
    // handles both).
    ctx.register_class("OxPHP\\Shared\\DeadlockException")
        .extends("OxPHP\\Shared\\TimeoutException")
        .build()?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventDispatcher;
    use crate::plugin::context::PluginDecoratorDef;
    use crate::plugin::handler::{PluginInternalHandler, PluginMetricsCollector};
    use crate::plugin::php::PluginNativeFunctionDef;
    use std::collections::HashMap;

    fn run_register() -> Vec<crate::plugin::builders::definitions::PhpClassDef> {
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
            "ox_shared".into(),
            "__oxp_shared_".into(),
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

        register_all(&mut ctx).unwrap();
        drop(ctx);
        php_classes
    }

    #[test]
    fn all_ten_classes_registered() {
        let classes = run_register();
        assert_eq!(classes.len(), 10);
    }

    #[test]
    fn base_extends_php_exception() {
        let classes = run_register();
        let base = classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Shared\\Exception")
            .unwrap();
        assert_eq!(base.parent.as_deref(), Some("Exception"));
    }

    #[test]
    fn cycle_extends_type() {
        let classes = run_register();
        let cycle = classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Shared\\CycleException")
            .unwrap();
        assert_eq!(
            cycle.parent.as_deref(),
            Some("OxPHP\\Shared\\TypeException")
        );
    }

    #[test]
    fn deadlock_extends_timeout() {
        let classes = run_register();
        let dl = classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Shared\\DeadlockException")
            .unwrap();
        assert_eq!(
            dl.parent.as_deref(),
            Some("OxPHP\\Shared\\TimeoutException")
        );
    }
}
