use crate::plugin::types::*;
use crate::plugin::PluginError;

use super::definitions::{PhpConstantDef, PhpInterfaceDef, PhpMethodDef, PhpParamDef};

// ─── InterfaceBuilder ─────────────────────────────────────────────────────────

pub struct InterfaceBuilder<'a> {
    def: PhpInterfaceDef,
    target: &'a mut Vec<PhpInterfaceDef>,
}

impl<'a> InterfaceBuilder<'a> {
    pub(crate) fn new(
        fqn: &str,
        plugin_name: &str,
        target: &'a mut Vec<PhpInterfaceDef>,
    ) -> Self {
        let mut def = PhpInterfaceDef::new(fqn);
        def.plugin_name = plugin_name.to_string();
        Self { def, target }
    }

    /// Set the parent interface FQN (single inheritance for interfaces).
    pub fn extends(mut self, parent_fqn: &str) -> Self {
        self.def.parent = Some(parent_fqn.to_string());
        self
    }

    /// Add an interface constant.
    pub fn constant(mut self, name: &str, value: PhpValue, visibility: Visibility) -> Self {
        self.def.constants.push(PhpConstantDef {
            name: name.to_string(),
            value,
            visibility,
        });
        self
    }

    /// Start building an interface method. Transfers ownership to `InterfaceMethodBuilder`.
    pub fn method(self, name: &str) -> InterfaceMethodBuilder<'a> {
        InterfaceMethodBuilder {
            parent: self,
            method_def: PhpMethodDef::new(name),
        }
    }

    /// Push the interface definition to the target collection.
    pub fn build(self) -> Result<(), PluginError> {
        self.target.push(self.def);
        Ok(())
    }
}

// ─── InterfaceMethodBuilder ───────────────────────────────────────────────────

pub struct InterfaceMethodBuilder<'a> {
    parent: InterfaceBuilder<'a>,
    method_def: PhpMethodDef,
}

impl<'a> InterfaceMethodBuilder<'a> {
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

    /// Mark the method as static.
    pub fn static_(mut self) -> Self {
        self.method_def.modifiers |= Modifiers::STATIC;
        self
    }

    /// Finalize the method: sets ABSTRACT modifier, pushes to parent, returns parent builder.
    pub fn done(mut self) -> InterfaceBuilder<'a> {
        self.method_def.modifiers |= Modifiers::ABSTRACT;
        self.parent.def.methods.push(self.method_def);
        self.parent
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_interface(f: impl FnOnce(InterfaceBuilder<'_>)) -> PhpInterfaceDef {
        let mut interfaces = Vec::new();
        let builder = InterfaceBuilder::new("Test\\MyInterface", "test_plugin", &mut interfaces);
        f(builder);
        assert_eq!(interfaces.len(), 1);
        interfaces.pop().unwrap()
    }

    #[test]
    fn test_minimal_interface() {
        let iface = collect_interface(|b| {
            b.build().unwrap();
        });
        assert_eq!(iface.fqn, "Test\\MyInterface");
        assert_eq!(iface.plugin_name, "test_plugin");
        assert!(iface.parent.is_none());
        assert!(iface.constants.is_empty());
        assert!(iface.methods.is_empty());
    }

    #[test]
    fn test_interface_extends() {
        let iface = collect_interface(|b| {
            b.extends("Countable").build().unwrap();
        });
        assert_eq!(iface.parent, Some("Countable".to_string()));
    }

    #[test]
    fn test_interface_methods() {
        // Two methods — one static, one not.
        let iface = collect_interface(|b| {
            b.method("count")
                .returns(PhpType::Int)
                .done()
                .method("create")
                .static_()
                .returns(PhpType::Self_)
                .done()
                .build()
                .unwrap();
        });
        assert_eq!(iface.methods.len(), 2);

        let count_m = &iface.methods[0];
        assert_eq!(count_m.name, "count");
        assert_eq!(count_m.return_type, Some(PhpType::Int));
        assert!(count_m.modifiers.contains(Modifiers::ABSTRACT));
        assert!(!count_m.modifiers.contains(Modifiers::STATIC));

        let create_m = &iface.methods[1];
        assert_eq!(create_m.name, "create");
        assert_eq!(create_m.return_type, Some(PhpType::Self_));
        assert!(create_m.modifiers.contains(Modifiers::ABSTRACT));
        assert!(create_m.modifiers.contains(Modifiers::STATIC));
    }

    #[test]
    fn test_interface_constant() {
        let iface = collect_interface(|b| {
            b.constant("VERSION", PhpValue::String("1.0.0".to_string()), Visibility::Public)
                .build()
                .unwrap();
        });
        assert_eq!(iface.constants.len(), 1);
        let c = &iface.constants[0];
        assert_eq!(c.name, "VERSION");
        assert_eq!(c.value, PhpValue::String("1.0.0".to_string()));
        assert_eq!(c.visibility, Visibility::Public);
    }

    #[test]
    fn test_interface_method_optional_param() {
        let iface = collect_interface(|b| {
            b.method("greet")
                .param("name", PhpType::String)
                .optional_param("title", PhpType::String, PhpValue::Null)
                .returns(PhpType::String)
                .done()
                .build()
                .unwrap();
        });
        let m = &iface.methods[0];
        assert_eq!(m.params.len(), 2);
        assert_eq!(m.params[0].name, "name");
        assert!(m.params[0].required);
        assert_eq!(m.params[1].name, "title");
        assert!(!m.params[1].required);
    }

    #[test]
    fn test_interface_method_variadic() {
        let iface = collect_interface(|b| {
            b.method("log")
                .variadic_param("messages", PhpType::String)
                .done()
                .build()
                .unwrap();
        });
        let m = &iface.methods[0];
        assert!(m.is_variadic);
        assert_eq!(m.params.len(), 1);
        assert!(m.params[0].is_variadic);
        assert!(m.modifiers.contains(Modifiers::ABSTRACT));
    }
}
