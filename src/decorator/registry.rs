use std::os::raw::c_void;
use std::sync::Arc;

use dashmap::DashMap;

#[cfg(feature = "php")]
use super::types::AttrArg;
use super::types::{AttrArgs, AttributeTargets, Decorator};

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
    /// `attr_ctx` is the opaque C-side attribute resolver context used to read
    /// each matched decorator's constructor arguments (null in host builds).
    pub fn resolve(&self, fn_id: usize, attr_names: &[String], attr_ctx: *mut c_void) -> bool {
        #[cfg(not(feature = "php"))]
        let _ = attr_ctx;
        let mut decorators = Vec::new();

        for (index, name) in attr_names.iter().enumerate() {
            if let Some(dec) = self.rust_decorators.get(name) {
                let template = Arc::clone(dec.value());
                // Decode the attribute's constructor args once and let the
                // decorator build a configured instance. `configure`
                // returning None means "no per-attribute config" — share
                // the registered template as-is.
                #[cfg(feature = "php")]
                let args = read_attr_args(attr_ctx, name);
                #[cfg(not(feature = "php"))]
                let args = AttrArgs::default();
                let inst = template.configure(&args).unwrap_or(template);
                decorators.push(ResolvedDecorator::Rust(inst));
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

    /// Returns the number of registered Rust-native decorators.
    pub fn rust_decorator_count(&self) -> usize {
        self.rust_decorators.len()
    }

    /// Get the number of PHP decorators resolved for a function.
    pub fn php_decorator_count(&self, fn_id: usize) -> usize {
        match self.resolved.get(&fn_id) {
            Some(resolved) => resolved
                .iter()
                .filter(|d| matches!(d, ResolvedDecorator::Php { .. }))
                .count(),
            None => 0,
        }
    }

    /// Get PHP decorator info at a given index (only counting PHP decorators).
    /// Returns (class_name, cache_key) or None.
    pub fn php_decorator_at(&self, fn_id: usize, php_index: usize) -> Option<(String, u64)> {
        match self.resolved.get(&fn_id) {
            Some(resolved) => {
                let mut count = 0;
                for dec in resolved.iter() {
                    if let ResolvedDecorator::Php {
                        class_name,
                        cache_key,
                    } = dec
                    {
                        if count == php_index {
                            return Some((class_name.clone(), *cache_key));
                        }
                        count += 1;
                    }
                }
                None
            }
            None => None,
        }
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

/// Decode the constructor arguments of attribute `attr_name` from the
/// C-side resolver context. Tries the function scope first, then the
/// class scope (matching how decorator attributes attach). Reads the
/// first occurrence only. Returns empty args when the context is null
/// or the attribute carries no arguments.
#[cfg(feature = "php")]
fn read_attr_args(attr_ctx: *mut c_void, attr_name: &str) -> AttrArgs {
    use std::ffi::CString;

    if attr_ctx.is_null() {
        return AttrArgs::default();
    }
    let cname = match CString::new(attr_name) {
        Ok(c) => c,
        Err(_) => return AttrArgs::default(),
    };

    for is_class_scope in [0_i32, 1_i32] {
        let count = unsafe {
            crate::php::bindings::oxphp_bridge_attr_arg_count(
                attr_ctx,
                is_class_scope,
                cname.as_ptr(),
                0, // first occurrence
            )
        };
        if count < 0 {
            continue; // attribute not present in this scope
        }
        let mut args = Vec::with_capacity(count as usize);
        for arg_idx in 0..count as u32 {
            args.push(read_one_arg(
                attr_ctx,
                is_class_scope,
                cname.as_ptr(),
                arg_idx,
            ));
        }
        return AttrArgs::from_pairs(args);
    }
    AttrArgs::default()
}

/// Read a single attribute argument as a `(name, value)` pair via the
/// bridge. `name` is `Some` for `name:`-style arguments, `None` for
/// positional ones — see [`AttrArgs`] for why both are surfaced.
#[cfg(feature = "php")]
fn read_one_arg(
    attr_ctx: *mut c_void,
    is_class_scope: i32,
    attr_name: *const std::os::raw::c_char,
    arg_idx: u32,
) -> (Option<Arc<str>>, AttrArg) {
    let mut out_long: i64 = 0;
    let mut out_double: f64 = 0.0;
    let mut out_bool: i32 = 0;
    let mut buf = [0 as std::os::raw::c_char; 256];
    let kind = unsafe {
        crate::php::bindings::oxphp_bridge_read_attr_arg_variant(
            attr_ctx,
            is_class_scope,
            attr_name,
            0, // first occurrence
            arg_idx,
            &mut out_long,
            &mut out_double,
            &mut out_bool,
            buf.as_mut_ptr(),
            buf.len(),
        )
    };
    let value = match kind {
        1 => AttrArg::Int(out_long),
        2 => AttrArg::Float(out_double),
        3 => {
            let s = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
            AttrArg::Str(Arc::from(s.as_ref()))
        }
        4 => AttrArg::Bool(out_bool != 0),
        // tag 5 = explicit null; tag 0 = decode error. Both map to Null:
        // a typed accessor then returns None and the decorator falls back
        // to its default. Conflating them is safe here because PHP requires
        // attribute arguments to be constant expressions, so a decode error
        // (zend_get_attribute_value != SUCCESS) is effectively unreachable
        // for valid source — there is no distinct "error" outcome worth
        // surfacing separately.
        _ => AttrArg::Null,
    };
    (
        read_arg_name(attr_ctx, is_class_scope, attr_name, arg_idx),
        value,
    )
}

/// Read the parameter name of one attribute argument. Returns `None`
/// for a positional argument (no `name:`) or an empty/oversized name.
#[cfg(feature = "php")]
fn read_arg_name(
    attr_ctx: *mut c_void,
    is_class_scope: i32,
    attr_name: *const std::os::raw::c_char,
    arg_idx: u32,
) -> Option<Arc<str>> {
    let mut buf = [0 as std::os::raw::c_char; 128];
    let len = unsafe {
        crate::php::bindings::oxphp_bridge_read_attr_arg_name(
            attr_ctx,
            is_class_scope,
            attr_name,
            0, // first occurrence
            arg_idx,
            buf.as_mut_ptr(),
            buf.len(),
        )
    };
    if len == 0 {
        return None;
    }
    let s = unsafe { std::ffi::CStr::from_ptr(buf.as_ptr()) }.to_string_lossy();
    Some(Arc::from(s.as_ref()))
}

#[cfg(test)]
mod tests {
    use super::super::types::{DecoratorAction, DecoratorCallContext, DecoratorCallResult};
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

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

    /// Decorator whose `configure` returns a distinct instance. `on_begin`
    /// bumps `template_begins` on the registered template and
    /// `configured_begins` on a configured instance, so a test can tell
    /// which instance `resolve` cached.
    struct ConfigurableMock {
        configured: bool,
        template_begins: Arc<AtomicU32>,
        configured_begins: Arc<AtomicU32>,
    }

    impl Decorator for ConfigurableMock {
        fn attribute_name(&self) -> &str {
            "App\\Cfg"
        }
        fn targets(&self) -> AttributeTargets {
            AttributeTargets::ALL
        }
        fn on_begin(&self, _: &DecoratorCallContext) -> DecoratorAction {
            if self.configured {
                self.configured_begins.fetch_add(1, Ordering::Relaxed);
            } else {
                self.template_begins.fetch_add(1, Ordering::Relaxed);
            }
            DecoratorAction::Continue
        }
        fn on_end(&self, _: &DecoratorCallContext, _: &DecoratorCallResult) {}
        fn configure(&self, _args: &AttrArgs) -> Option<Arc<dyn Decorator>> {
            Some(Arc::new(ConfigurableMock {
                configured: true,
                template_begins: Arc::clone(&self.template_begins),
                configured_begins: Arc::clone(&self.configured_begins),
            }))
        }
    }

    #[test]
    fn test_resolve_caches_configured_instance() {
        let registry = DecoratorRegistry::new();
        let template_begins = Arc::new(AtomicU32::new(0));
        let configured_begins = Arc::new(AtomicU32::new(0));
        registry.register_rust(Arc::new(ConfigurableMock {
            configured: false,
            template_begins: Arc::clone(&template_begins),
            configured_begins: Arc::clone(&configured_begins),
        }));

        let attrs = vec!["App\\Cfg".to_string()];
        assert!(registry.resolve(0xc0, &attrs, std::ptr::null_mut()));

        let resolved = registry.get_resolved(0xc0).unwrap();
        let ctx = DecoratorCallContext {
            target: Arc::from("f"),
            class: None,
            method: None,
            function: Some(Arc::from("f")),
            object_id: 0,
            request_id: String::new(),
            trace_id: String::new(),
            timestamp_ns: 0,
        };
        for d in resolved.iter() {
            if let ResolvedDecorator::Rust(dec) = d {
                dec.on_begin(&ctx);
            }
        }
        // The configured instance ran, not the registered template.
        assert_eq!(configured_begins.load(Ordering::Relaxed), 1);
        assert_eq!(template_begins.load(Ordering::Relaxed), 0);
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
        let found = registry.resolve(42, &attrs, std::ptr::null_mut());
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
        let found = registry.resolve(99, &attrs, std::ptr::null_mut());
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
        registry.resolve(7, &attrs, std::ptr::null_mut());

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
        registry.resolve(1, &attrs, std::ptr::null_mut());
        assert!(registry.get_resolved(1).is_some());

        registry.clear_cache();

        // Cache entry is gone.
        assert!(registry.get_resolved(1).is_none());
        // Registrations are still present.
        assert!(registry.is_registered("App\\Timer"));
        assert!(registry.is_registered("App\\Cache"));
    }

    #[test]
    fn test_php_decorator_count() {
        let registry = DecoratorRegistry::new();
        let rust_dec = Arc::new(MockDecorator {
            name: "App\\Timer",
            targets: AttributeTargets::FUNCTION,
        });
        registry.register_rust(rust_dec);
        registry.register_php("App\\Cache".to_string(), AttributeTargets::METHOD);
        registry.register_php("App\\Log".to_string(), AttributeTargets::FUNCTION);

        let attrs = vec![
            "App\\Timer".to_string(),
            "App\\Cache".to_string(),
            "App\\Log".to_string(),
        ];
        registry.resolve(10, &attrs, std::ptr::null_mut());

        // Only PHP decorators counted (App\\Cache and App\\Log)
        assert_eq!(registry.php_decorator_count(10), 2);
        // Non-existent fn_id returns 0
        assert_eq!(registry.php_decorator_count(999), 0);
    }

    #[test]
    fn test_php_decorator_at() {
        let registry = DecoratorRegistry::new();
        let rust_dec = Arc::new(MockDecorator {
            name: "App\\Timer",
            targets: AttributeTargets::FUNCTION,
        });
        registry.register_rust(rust_dec);
        registry.register_php("App\\Cache".to_string(), AttributeTargets::METHOD);
        registry.register_php("App\\Log".to_string(), AttributeTargets::FUNCTION);

        let attrs = vec![
            "App\\Timer".to_string(),
            "App\\Cache".to_string(),
            "App\\Log".to_string(),
        ];
        registry.resolve(20, &attrs, std::ptr::null_mut());

        // Index 0 = first PHP decorator (App\\Cache)
        let (name, _key) = registry.php_decorator_at(20, 0).unwrap();
        assert_eq!(name, "App\\Cache");

        // Index 1 = second PHP decorator (App\\Log)
        let (name, _key) = registry.php_decorator_at(20, 1).unwrap();
        assert_eq!(name, "App\\Log");

        // Index 2 = out of bounds
        assert!(registry.php_decorator_at(20, 2).is_none());

        // Non-existent fn_id
        assert!(registry.php_decorator_at(999, 0).is_none());
    }

    #[test]
    fn test_php_decorator_count_no_php_decorators() {
        let registry = DecoratorRegistry::new();
        let rust_dec = Arc::new(MockDecorator {
            name: "App\\Timer",
            targets: AttributeTargets::ALL,
        });
        registry.register_rust(rust_dec);

        let attrs = vec!["App\\Timer".to_string()];
        registry.resolve(30, &attrs, std::ptr::null_mut());

        assert_eq!(registry.php_decorator_count(30), 0);
        assert!(registry.php_decorator_at(30, 0).is_none());
    }
}
