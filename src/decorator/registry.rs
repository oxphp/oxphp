use std::sync::Arc;

use dashmap::DashMap;

use super::types::{AttributeTargets, Decorator};

/// Metadata for a PHP-side decorator registered via `oxphp_register_decorator()`.
#[derive(Debug, Clone)]
pub struct PhpDecoratorMeta {
    pub class_name: String,
    pub targets: AttributeTargets,
}

/// A resolved decorator for a specific function — either Rust-native or PHP.
#[derive(Clone)]
pub enum ResolvedDecorator {
    Rust(Arc<dyn Decorator>),
    /// cache_key is an opaque index into the per-thread C-side instance cache.
    Php {
        class_name: String,
        cache_key: u64,
    },
}

/// Central registry for all decorator registrations.
/// Thread-safe via DashMap — accessed from multiple PHP worker threads.
pub struct DecoratorRegistry {
    rust_decorators: DashMap<String, Arc<dyn Decorator>>,
    php_decorators: DashMap<String, PhpDecoratorMeta>,
    resolved: DashMap<usize, Vec<ResolvedDecorator>>,
}

impl DecoratorRegistry {
    pub fn new() -> Self {
        Self {
            rust_decorators: DashMap::new(),
            php_decorators: DashMap::new(),
            resolved: DashMap::new(),
        }
    }

    /// Register a Rust-native decorator (called by plugin init).
    pub fn register_rust(&self, decorator: Arc<dyn Decorator>) {
        let name = decorator.attribute_name().to_string();
        self.rust_decorators.insert(name, decorator);
    }

    /// Register a PHP-side decorator (called via bridge from oxphp_register_decorator).
    pub fn register_php(&self, class_name: String, targets: AttributeTargets) {
        self.php_decorators.insert(
            class_name.clone(),
            PhpDecoratorMeta {
                class_name,
                targets,
            },
        );
    }

    /// Check if an attribute name is registered as a decorator (either Rust or PHP).
    pub fn is_registered(&self, attribute_name: &str) -> bool {
        self.rust_decorators.contains_key(attribute_name)
            || self.php_decorators.contains_key(attribute_name)
    }

    /// Resolve decorators for a function ID. Returns true if decorators were found.
    /// Called by observer init — caches the result for subsequent calls.
    /// `attr_names` is the list of attribute class names found on the function.
    pub fn resolve(&self, fn_id: usize, attr_names: &[String]) -> bool {
        let mut decorators = Vec::new();

        for (index, name) in attr_names.iter().enumerate() {
            if let Some(dec) = self.rust_decorators.get(name) {
                decorators.push(ResolvedDecorator::Rust(Arc::clone(dec.value())));
            } else if self.php_decorators.contains_key(name) {
                decorators.push(ResolvedDecorator::Php {
                    class_name: name.clone(),
                    cache_key: index as u64,
                });
            }
            // Unknown attributes (built-in PHP, unregistered) — silently skip.
        }

        let found = !decorators.is_empty();
        if found {
            self.resolved.insert(fn_id, decorators);
        }
        found
    }

    /// Get resolved decorators for a function ID.
    pub fn get_resolved(
        &self,
        fn_id: usize,
    ) -> Option<dashmap::mapref::one::Ref<'_, usize, Vec<ResolvedDecorator>>> {
        self.resolved.get(&fn_id)
    }

    /// Clear resolution cache (called at RSHUTDOWN in traditional mode).
    pub fn clear_cache(&self) {
        self.resolved.clear();
    }
}

impl Default for DecoratorRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::types::{DecoratorAction, DecoratorCallContext, DecoratorCallResult};
    use super::*;

    struct MockDecorator {
        name: &'static str,
        targets: AttributeTargets,
    }

    impl Decorator for MockDecorator {
        fn attribute_name(&self) -> &str {
            self.name
        }
        fn targets(&self) -> AttributeTargets {
            self.targets
        }
        fn on_begin(&self, _: &DecoratorCallContext) -> DecoratorAction {
            DecoratorAction::Continue
        }
        fn on_end(&self, _: &DecoratorCallContext, _: &DecoratorCallResult) {}
    }

    #[test]
    fn test_register_rust_decorator() {
        let registry = DecoratorRegistry::new();
        let dec = Arc::new(MockDecorator {
            name: "App\\Timer",
            targets: AttributeTargets::FUNCTION,
        });
        registry.register_rust(dec);
        assert!(registry.is_registered("App\\Timer"));
        assert!(!registry.is_registered("App\\Unknown"));
    }

    #[test]
    fn test_register_php_decorator() {
        let registry = DecoratorRegistry::new();
        registry.register_php("App\\Cache".to_string(), AttributeTargets::METHOD);
        assert!(registry.is_registered("App\\Cache"));
        assert!(!registry.is_registered("App\\Other"));
    }

    #[test]
    fn test_resolve_finds_registered_decorators() {
        let registry = DecoratorRegistry::new();

        let rust_dec = Arc::new(MockDecorator {
            name: "App\\Timer",
            targets: AttributeTargets::FUNCTION,
        });
        registry.register_rust(rust_dec);
        registry.register_php("App\\Cache".to_string(), AttributeTargets::METHOD);

        let attrs = vec![
            "App\\Timer".to_string(),
            "App\\Cache".to_string(),
            "App\\Unknown".to_string(),
        ];
        let found = registry.resolve(42, &attrs);
        assert!(found);

        let resolved = registry.get_resolved(42).unwrap();
        assert_eq!(resolved.len(), 2);

        assert!(matches!(resolved[0], ResolvedDecorator::Rust(_)));
        assert!(matches!(
            resolved[1],
            ResolvedDecorator::Php { ref class_name, .. } if class_name == "App\\Cache"
        ));
    }

    #[test]
    fn test_resolve_no_decorators_returns_false() {
        let registry = DecoratorRegistry::new();
        let attrs = vec!["PHP\\Attribute".to_string(), "App\\Unknown".to_string()];
        let found = registry.resolve(99, &attrs);
        assert!(!found);
        assert!(registry.get_resolved(99).is_none());
    }

    #[test]
    fn test_resolve_caches_result() {
        let registry = DecoratorRegistry::new();
        let dec = Arc::new(MockDecorator {
            name: "App\\Profiler",
            targets: AttributeTargets::ALL,
        });
        registry.register_rust(dec);

        let attrs = vec!["App\\Profiler".to_string()];
        registry.resolve(7, &attrs);

        let cached = registry.get_resolved(7);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().len(), 1);
    }

    #[test]
    fn test_clear_cache() {
        let registry = DecoratorRegistry::new();
        let dec = Arc::new(MockDecorator {
            name: "App\\Timer",
            targets: AttributeTargets::FUNCTION,
        });
        registry.register_rust(dec);
        registry.register_php("App\\Cache".to_string(), AttributeTargets::METHOD);

        let attrs = vec!["App\\Timer".to_string()];
        registry.resolve(1, &attrs);
        assert!(registry.get_resolved(1).is_some());

        registry.clear_cache();

        // Cache entry is gone.
        assert!(registry.get_resolved(1).is_none());
        // Registrations are still present.
        assert!(registry.is_registered("App\\Timer"));
        assert!(registry.is_registered("App\\Cache"));
    }
}
