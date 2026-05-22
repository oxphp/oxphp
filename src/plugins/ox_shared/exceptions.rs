//! Register all Shared\* exception classes at plugin init.
//!
//! Hierarchy (13 classes total — 10 Shared\* + 3 Shared\* extending Async\*):
//!
//!   \Exception
//!     ├── OxPHP\Async\AsyncException                  (registered by ox_async plugin)
//!     │    ├── OxPHP\Shared\OperationTimeoutException  recvTimeout / sendTimeout / withLockTimeout
//!     │    ├── OxPHP\Shared\ContentionException        tryWithLock contention
//!     │    └── OxPHP\Shared\DeadlockException          reentrant / wait-for-cycle
//!     └── OxPHP\Shared\SharedException
//!          ├── StaleHandleException
//!          ├── TypeException
//!          │    └── CycleException                     Map::set would form a cycle
//!          ├── CapacityException
//!          ├── ClosedException                         DEPRECATED — still thrown by Pool;
//!          │                                            removed once Pool migrates
//!          ├── PoisonedException                       Once Poison-mode failure
//!          │                                            (first-class, not deprecated)
//!          ├── UninitializedException
//!          ├── InvalidOrderingException                Atomic op got an invalid memory ordering
//!          └── CorruptedMutexException                 Rust panic crossed FFI; mutex unusable
//!
//! Cross-plugin coupling: the three Shared\* classes that extend
//! Async\AsyncException require the ox_async plugin's exceptions
//! to be registered before ox_shared::init runs.

use crate::plugin::{PluginContext, PluginError};

pub fn register_all(ctx: &mut PluginContext) -> Result<(), PluginError> {
    // Base.
    ctx.register_class("OxPHP\\Shared\\SharedException")
        .extends("Exception")
        .build()?;

    // Direct subclasses of SharedException.
    // NOTE: ClosedException is deprecated but kept because Shared\Pool
    // still throws it; it is scheduled for removal once Pool migrates to
    // the new result/exception model. PoisonedException is NOT deprecated —
    // Shared\Once throws it as a first-class part of its Poison failure mode.
    for child in [
        "OxPHP\\Shared\\StaleHandleException",
        "OxPHP\\Shared\\TypeException",
        "OxPHP\\Shared\\CapacityException",
        "OxPHP\\Shared\\ClosedException",
        "OxPHP\\Shared\\PoisonedException",
        "OxPHP\\Shared\\UninitializedException",
        "OxPHP\\Shared\\InvalidOrderingException",
        "OxPHP\\Shared\\CorruptedMutexException",
    ] {
        ctx.register_class(child)
            .extends("OxPHP\\Shared\\SharedException")
            .build()?;
    }

    // CycleException extends TypeException (a cycle is a value-shape
    // violation at the storage slot).
    ctx.register_class("OxPHP\\Shared\\CycleException")
        .extends("OxPHP\\Shared\\TypeException")
        .build()?;

    // Cross-plugin: OperationTimeoutException, ContentionException, and
    // DeadlockException extend Async\AsyncException so `catch (AsyncException)`
    // sweeps every concurrency-related condition across the Shared\* and
    // Async\* surfaces.
    for child in [
        "OxPHP\\Shared\\OperationTimeoutException",
        "OxPHP\\Shared\\ContentionException",
        "OxPHP\\Shared\\DeadlockException",
    ] {
        ctx.register_class(child)
            .extends("OxPHP\\Async\\AsyncException")
            .build()?;
    }

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
    fn all_thirteen_classes_registered() {
        let classes = run_register();
        // 1 base (SharedException) + 8 direct children + CycleException
        // + 3 AsyncException children = 13.
        assert_eq!(classes.len(), 13);
        let fqns: Vec<&str> = classes.iter().map(|c| c.fqn.as_str()).collect();
        for required in [
            "OxPHP\\Shared\\SharedException",
            "OxPHP\\Shared\\StaleHandleException",
            "OxPHP\\Shared\\TypeException",
            "OxPHP\\Shared\\CycleException",
            "OxPHP\\Shared\\CapacityException",
            "OxPHP\\Shared\\ClosedException",
            "OxPHP\\Shared\\PoisonedException",
            "OxPHP\\Shared\\UninitializedException",
            "OxPHP\\Shared\\InvalidOrderingException",
            "OxPHP\\Shared\\CorruptedMutexException",
            "OxPHP\\Shared\\OperationTimeoutException",
            "OxPHP\\Shared\\ContentionException",
            "OxPHP\\Shared\\DeadlockException",
        ] {
            assert!(fqns.contains(&required), "missing class: {required}");
        }
        // The pre-refactor Shared\TimeoutException MUST be gone.
        assert!(
            !fqns.contains(&"OxPHP\\Shared\\TimeoutException"),
            "Shared\\TimeoutException should have been removed"
        );
    }

    #[test]
    fn base_extends_php_exception() {
        let classes = run_register();
        let base = classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Shared\\SharedException")
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
    fn deadlock_extends_async_exception() {
        let classes = run_register();
        let dl = classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Shared\\DeadlockException")
            .unwrap();
        assert_eq!(
            dl.parent.as_deref(),
            Some("OxPHP\\Async\\AsyncException"),
            "DeadlockException must reparent to Async\\AsyncException (was Shared\\TimeoutException)"
        );
    }

    #[test]
    fn operation_timeout_extends_async_exception() {
        let classes = run_register();
        let ot = classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Shared\\OperationTimeoutException")
            .unwrap();
        assert_eq!(ot.parent.as_deref(), Some("OxPHP\\Async\\AsyncException"));
    }

    #[test]
    fn contention_extends_async_exception() {
        let classes = run_register();
        let c = classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Shared\\ContentionException")
            .unwrap();
        assert_eq!(c.parent.as_deref(), Some("OxPHP\\Async\\AsyncException"));
    }

    #[test]
    fn corrupted_mutex_extends_shared_exception() {
        let classes = run_register();
        let cm = classes
            .iter()
            .find(|c| c.fqn == "OxPHP\\Shared\\CorruptedMutexException")
            .unwrap();
        assert_eq!(cm.parent.as_deref(), Some("OxPHP\\Shared\\SharedException"));
    }
}
