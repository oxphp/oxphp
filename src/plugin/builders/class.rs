use std::os::raw::c_void;

use crate::bridge::call::NativeCall;
use crate::plugin::php::PhpError;
use crate::plugin::types::*;
use crate::plugin::PluginError;

use super::definitions::{
    MagicHandler, PhpClassDef, PhpConstantDef, PhpMethodDef, PhpParamDef, PhpPropertyDef,
};

// ─── ClassBuilder ─────────────────────────────────────────────────────────────

pub struct ClassBuilder<'a> {
    def: PhpClassDef,
    target: &'a mut Vec<PhpClassDef>,
}

impl<'a> ClassBuilder<'a> {
    pub(crate) fn new(fqn: &str, plugin_name: &str, target: &'a mut Vec<PhpClassDef>) -> Self {
        let mut def = PhpClassDef::new(fqn);
        def.plugin_name = plugin_name.to_string();
        Self { def, target }
    }

    /// Set the parent class FQN.
    pub fn extends(mut self, parent_fqn: &str) -> Self {
        self.def.parent = Some(parent_fqn.to_string());
        self
    }

    /// Add an implemented interface FQN (can be called multiple times).
    pub fn implements(mut self, interface_fqn: &str) -> Self {
        self.def.interfaces.push(interface_fqn.to_string());
        self
    }

    /// Mark the class as abstract.
    pub fn abstract_(mut self) -> Self {
        self.def.modifiers |= Modifiers::ABSTRACT;
        self
    }

    /// Mark the class as final.
    pub fn final_(mut self) -> Self {
        self.def.modifiers |= Modifiers::FINAL;
        self
    }

    /// Mark the class as readonly (PHP 8.2+).
    pub fn readonly(mut self) -> Self {
        self.def.modifiers |= Modifiers::READONLY;
        self
    }

    /// Add a property with default modifiers and no default value.
    pub fn property(self, name: &str, php_type: PhpType, visibility: Visibility) -> Self {
        self.property_with(name, php_type, visibility, Modifiers::empty(), None)
    }

    /// Add a property with full control over modifiers and default.
    pub fn property_with(
        mut self,
        name: &str,
        php_type: PhpType,
        visibility: Visibility,
        modifiers: Modifiers,
        default: Option<PhpValue>,
    ) -> Self {
        self.def.properties.push(PhpPropertyDef {
            name: name.to_string(),
            php_type,
            visibility,
            modifiers,
            default,
        });
        self
    }

    /// Add a class constant.
    pub fn constant(mut self, name: &str, value: PhpValue, visibility: Visibility) -> Self {
        self.def.constants.push(PhpConstantDef {
            name: name.to_string(),
            value,
            visibility,
        });
        self
    }

    /// Start building a method. Transfers ownership to `MethodBuilder`.
    pub fn method(self, name: &str) -> MethodBuilder<'a> {
        MethodBuilder {
            parent: self,
            method_def: PhpMethodDef::new(name),
        }
    }

    /// Shortcut for `method("__construct")`.
    pub fn constructor(self) -> MethodBuilder<'a> {
        self.method("__construct")
    }

    /// Attach custom per-instance storage. Sets `has_custom_storage = true`,
    /// `storage_factory` (allocates via `factory()`), and `storage_drop` (frees via `Box::from_raw`).
    pub fn with_storage<T: Send + Sync + 'static>(
        mut self,
        factory: impl Fn() -> T + Send + Sync + 'static,
    ) -> Self {
        self.def.has_custom_storage = true;
        self.def.storage_factory = Some(Box::new(move || {
            Box::into_raw(Box::new(factory())) as *mut c_void
        }));
        self.def.storage_drop = Some(Box::new(|ptr| {
            // Safety: ptr was created by our storage_factory above.
            unsafe {
                drop(Box::from_raw(ptr as *mut T));
            }
        }));
        self.def.storage_clone = None;
        self
    }

    /// Start building a magic method handler. Transfers ownership to `MagicBuilder`.
    pub fn magic(self, magic: MagicMethod) -> MagicBuilder<'a> {
        MagicBuilder { parent: self, magic }
    }

    /// Validate and push the class definition to the target collection.
    pub fn build(self) -> Result<(), PluginError> {
        let def = &self.def;

        // 1. Abstract + final conflict.
        if def.modifiers.contains(Modifiers::ABSTRACT) && def.modifiers.contains(Modifiers::FINAL) {
            return Err(PluginError::Config(format!(
                "class '{}': cannot be both abstract and final",
                def.fqn
            )));
        }

        let is_abstract_class = def.modifiers.contains(Modifiers::ABSTRACT);

        for method in &def.methods {
            let is_abstract_method = method.modifiers.contains(Modifiers::ABSTRACT);

            // 2. Abstract methods only in abstract classes.
            if is_abstract_method && !is_abstract_class {
                return Err(PluginError::Config(format!(
                    "class '{}': method '{}' is abstract but class is not abstract",
                    def.fqn, method.name
                )));
            }

            // 3. Non-abstract methods must have handlers.
            if !is_abstract_method && method.handler.is_none() {
                return Err(PluginError::Config(format!(
                    "class '{}': method '{}' has no handler (use no_body() for abstract methods)",
                    def.fqn, method.name
                )));
            }
        }

        let _ = def;
        self.target.push(self.def);
        Ok(())
    }
}

// ─── MethodBuilder ────────────────────────────────────────────────────────────

pub struct MethodBuilder<'a> {
    parent: ClassBuilder<'a>,
    method_def: PhpMethodDef,
}

impl<'a> MethodBuilder<'a> {
    /// Set method visibility.
    pub fn visibility(mut self, v: Visibility) -> Self {
        self.method_def.visibility = v;
        self
    }

    /// Mark the method as static.
    pub fn static_(mut self) -> Self {
        self.method_def.modifiers |= Modifiers::STATIC;
        self
    }

    /// Mark the method as abstract.
    pub fn abstract_(mut self) -> Self {
        self.method_def.modifiers |= Modifiers::ABSTRACT;
        self
    }

    /// Mark the method as final.
    pub fn final_(mut self) -> Self {
        self.method_def.modifiers |= Modifiers::FINAL;
        self
    }

    /// Mark the method as async.
    pub fn async_(mut self) -> Self {
        self.method_def.is_async = true;
        self
    }

    /// Add a required parameter.
    pub fn param(mut self, name: &str, php_type: PhpType) -> Self {
        self.method_def.params.push(PhpParamDef::required(name, php_type));
        self
    }

    /// Add an optional parameter with a default value.
    pub fn optional_param(mut self, name: &str, php_type: PhpType, default: PhpValue) -> Self {
        self.method_def.params.push(PhpParamDef::optional(name, php_type, default));
        self
    }

    /// Add a variadic parameter. Also marks the method as variadic.
    pub fn variadic_param(mut self, name: &str, php_type: PhpType) -> Self {
        self.method_def.is_variadic = true;
        self.method_def.params.push(PhpParamDef::variadic(name, php_type));
        self
    }

    /// Set the return type.
    pub fn returns(mut self, php_type: PhpType) -> Self {
        self.method_def.return_type = Some(php_type);
        self
    }

    /// Attach a handler, push the method to the parent class, and return the parent builder.
    pub fn handler(
        mut self,
        f: impl Fn(&mut NativeCall) -> Result<(), PhpError> + Send + Sync + 'static,
    ) -> ClassBuilder<'a> {
        self.method_def.handler = Some(Box::new(f));
        self.parent.def.methods.push(self.method_def);
        self.parent
    }

    /// For abstract methods: push without a handler and return the parent builder.
    pub fn no_body(mut self) -> ClassBuilder<'a> {
        self.parent.def.methods.push(self.method_def);
        self.parent
    }
}

// ─── MagicBuilder ─────────────────────────────────────────────────────────────

pub struct MagicBuilder<'a> {
    parent: ClassBuilder<'a>,
    magic: MagicMethod,
}

impl<'a> MagicBuilder<'a> {
    /// Attach a handler in the parent's `magic_handlers` array and return the parent builder.
    pub fn handler(
        mut self,
        f: impl Fn(&mut NativeCall) -> Result<(), PhpError> + Send + Sync + 'static,
    ) -> ClassBuilder<'a> {
        let idx = self.magic.index();
        self.parent.def.magic_handlers[idx] = Some(Box::new(f) as MagicHandler);
        self.parent
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_class(f: impl FnOnce(ClassBuilder<'_>)) -> PhpClassDef {
        let mut classes = Vec::new();
        let builder = ClassBuilder::new("Test\\MyClass", "test_plugin", &mut classes);
        f(builder);
        assert_eq!(classes.len(), 1);
        classes.pop().unwrap()
    }

    #[test]
    fn test_minimal_class() {
        let cls = collect_class(|b| {
            b.build().unwrap();
        });
        assert_eq!(cls.fqn, "Test\\MyClass");
        assert_eq!(cls.plugin_name, "test_plugin");
    }

    #[test]
    fn test_class_extends_implements() {
        let cls = collect_class(|b| {
            b.extends("Base\\Class")
                .implements("Countable")
                .implements("Serializable")
                .build()
                .unwrap();
        });
        assert_eq!(cls.parent, Some("Base\\Class".to_string()));
        assert_eq!(cls.interfaces.len(), 2);
        assert!(cls.interfaces.contains(&"Countable".to_string()));
        assert!(cls.interfaces.contains(&"Serializable".to_string()));
    }

    #[test]
    fn test_class_modifiers() {
        let cls = collect_class(|b| {
            b.final_().readonly().build().unwrap();
        });
        assert!(cls.modifiers.contains(Modifiers::FINAL));
        assert!(cls.modifiers.contains(Modifiers::READONLY));
        assert!(!cls.modifiers.contains(Modifiers::ABSTRACT));
    }

    #[test]
    fn test_abstract_final_conflict() {
        let mut classes = Vec::new();
        let builder = ClassBuilder::new("Test\\MyClass", "test_plugin", &mut classes);
        let result = builder.abstract_().final_().build();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("abstract") && err.contains("final"));
    }

    #[test]
    fn test_class_property() {
        let cls = collect_class(|b| {
            b.property("name", PhpType::String, Visibility::Public)
                .property_with(
                    "count",
                    PhpType::Int,
                    Visibility::Protected,
                    Modifiers::READONLY,
                    Some(PhpValue::Int(0)),
                )
                .build()
                .unwrap();
        });
        assert_eq!(cls.properties.len(), 2);
        let name_prop = &cls.properties[0];
        assert_eq!(name_prop.name, "name");
        assert_eq!(name_prop.php_type, PhpType::String);
        assert_eq!(name_prop.visibility, Visibility::Public);
        assert_eq!(name_prop.modifiers, Modifiers::empty());
        assert!(name_prop.default.is_none());

        let count_prop = &cls.properties[1];
        assert_eq!(count_prop.name, "count");
        assert_eq!(count_prop.php_type, PhpType::Int);
        assert_eq!(count_prop.visibility, Visibility::Protected);
        assert!(count_prop.modifiers.contains(Modifiers::READONLY));
        assert_eq!(count_prop.default, Some(PhpValue::Int(0)));
    }

    #[test]
    fn test_class_constant() {
        let cls = collect_class(|b| {
            b.constant("VERSION", PhpValue::String("1.0.0".to_string()), Visibility::Public)
                .build()
                .unwrap();
        });
        assert_eq!(cls.constants.len(), 1);
        let c = &cls.constants[0];
        assert_eq!(c.name, "VERSION");
        assert_eq!(c.value, PhpValue::String("1.0.0".to_string()));
        assert_eq!(c.visibility, Visibility::Public);
    }

    #[test]
    fn test_class_method() {
        let cls = collect_class(|b| {
            b.method("greet")
                .param("name", PhpType::String)
                .optional_param("title", PhpType::String, PhpValue::Null)
                .returns(PhpType::String)
                .handler(|_call| Ok(()))
                .build()
                .unwrap();
        });
        assert_eq!(cls.methods.len(), 1);
        let m = &cls.methods[0];
        assert_eq!(m.name, "greet");
        assert_eq!(m.params.len(), 2);
        assert_eq!(m.params[0].name, "name");
        assert!(m.params[0].required);
        assert_eq!(m.params[1].name, "title");
        assert!(!m.params[1].required);
        assert_eq!(m.return_type, Some(PhpType::String));
        assert!(m.handler.is_some());
    }

    #[test]
    fn test_class_constructor_shortcut() {
        let cls = collect_class(|b| {
            b.constructor().handler(|_call| Ok(())).build().unwrap();
        });
        assert_eq!(cls.methods.len(), 1);
        assert_eq!(cls.methods[0].name, "__construct");
    }

    #[test]
    fn test_abstract_method_no_body() {
        let cls = collect_class(|b| {
            b.abstract_()
                .method("doIt")
                .abstract_()
                .no_body()
                .build()
                .unwrap();
        });
        assert_eq!(cls.methods.len(), 1);
        let m = &cls.methods[0];
        assert_eq!(m.name, "doIt");
        assert!(m.modifiers.contains(Modifiers::ABSTRACT));
        assert!(m.handler.is_none());
    }

    #[test]
    fn test_abstract_method_in_non_abstract_class_fails() {
        let mut classes = Vec::new();
        let builder = ClassBuilder::new("Test\\MyClass", "test_plugin", &mut classes);
        let result = builder
            .method("doIt")
            .abstract_()
            .no_body()
            .build();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("abstract"));
    }

    #[test]
    fn test_non_abstract_method_without_handler_fails() {
        let mut classes = Vec::new();
        let builder = ClassBuilder::new("Test\\MyClass", "test_plugin", &mut classes);
        let result = builder.method("doIt").no_body().build();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no handler"));
    }

    #[test]
    fn test_magic_method() {
        let cls = collect_class(|b| {
            b.magic(MagicMethod::ToString)
                .handler(|_call| Ok(()))
                .magic(MagicMethod::Clone)
                .handler(|_call| Ok(()))
                .build()
                .unwrap();
        });
        assert!(cls.magic_handlers[MagicMethod::ToString.index()].is_some());
        assert!(cls.magic_handlers[MagicMethod::Clone.index()].is_some());
        // Other slots should be None.
        assert!(cls.magic_handlers[MagicMethod::Get.index()].is_none());
    }

    #[test]
    fn test_with_storage() {
        let cls = collect_class(|b| {
            b.with_storage(|| 42u64).build().unwrap();
        });
        assert!(cls.has_custom_storage);
        assert!(cls.storage_factory.is_some());
        assert!(cls.storage_drop.is_some());
        assert!(cls.storage_clone.is_none());

        // Verify factory creates something non-null and drop doesn't crash.
        let factory = cls.storage_factory.as_ref().unwrap();
        let drop_fn = cls.storage_drop.as_ref().unwrap();
        let ptr = factory();
        assert!(!ptr.is_null());
        drop_fn(ptr);
    }

    #[test]
    fn test_method_static_final() {
        let cls = collect_class(|b| {
            b.method("create")
                .static_()
                .final_()
                .handler(|_call| Ok(()))
                .build()
                .unwrap();
        });
        let m = &cls.methods[0];
        assert!(m.modifiers.contains(Modifiers::STATIC));
        assert!(m.modifiers.contains(Modifiers::FINAL));
    }

    #[test]
    fn test_method_visibility() {
        let cls = collect_class(|b| {
            b.method("secret")
                .visibility(Visibility::Private)
                .handler(|_call| Ok(()))
                .build()
                .unwrap();
        });
        assert_eq!(cls.methods[0].visibility, Visibility::Private);
    }

    #[test]
    fn test_method_variadic_param() {
        let cls = collect_class(|b| {
            b.method("log")
                .variadic_param("messages", PhpType::String)
                .handler(|_call| Ok(()))
                .build()
                .unwrap();
        });
        let m = &cls.methods[0];
        assert!(m.is_variadic);
        assert_eq!(m.params.len(), 1);
        assert!(m.params[0].is_variadic);
        assert_eq!(m.params[0].name, "messages");
    }

    #[test]
    fn test_method_async_marker() {
        let cls = collect_class(|b| {
            b.method("fetchData")
                .async_()
                .handler(|_call| Ok(()))
                .build()
                .unwrap();
        });
        assert!(cls.methods[0].is_async);
    }
}
