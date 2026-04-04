use crate::bridge::call::NativeCall;
use crate::plugin::php::PhpError;
use crate::plugin::types::*;
use crate::plugin::PluginError;

use super::definitions::{PhpConstantDef, PhpEnumCaseDef, PhpEnumDef, PhpMethodDef, PhpParamDef};

// ─── EnumBuilder ──────────────────────────────────────────────────────────────

pub struct EnumBuilder<'a> {
    def: PhpEnumDef,
    target: &'a mut Vec<PhpEnumDef>,
}

impl<'a> EnumBuilder<'a> {
    pub(crate) fn new(fqn: &str, plugin_name: &str, target: &'a mut Vec<PhpEnumDef>) -> Self {
        let mut def = PhpEnumDef::new(fqn);
        def.plugin_name = plugin_name.to_string();
        Self { def, target }
    }

    /// Set the backing type (must be `PhpType::Int` or `PhpType::String`).
    pub fn backed_by(mut self, php_type: PhpType) -> Self {
        self.def.backing_type = Some(php_type);
        self
    }

    /// Add an implemented interface FQN (can be called multiple times).
    pub fn implements(mut self, interface_fqn: &str) -> Self {
        self.def.interfaces.push(interface_fqn.to_string());
        self
    }

    /// Add a unit (pure) enum case with no value.
    pub fn case(mut self, name: &str) -> Self {
        self.def.cases.push(PhpEnumCaseDef {
            name: name.to_string(),
            value: None,
        });
        self
    }

    /// Add a backed enum case with a value.
    pub fn case_value(mut self, name: &str, value: PhpValue) -> Self {
        self.def.cases.push(PhpEnumCaseDef {
            name: name.to_string(),
            value: Some(value),
        });
        self
    }

    /// Add an enum constant.
    pub fn constant(mut self, name: &str, value: PhpValue, visibility: Visibility) -> Self {
        self.def.constants.push(PhpConstantDef {
            name: name.to_string(),
            value,
            visibility,
        });
        self
    }

    /// Start building an enum method. Transfers ownership to `EnumMethodBuilder`.
    pub fn method(self, name: &str) -> EnumMethodBuilder<'a> {
        EnumMethodBuilder {
            parent: self,
            method_def: PhpMethodDef::new(name),
        }
    }

    /// Validate and push the enum definition to the target collection.
    ///
    /// Validation rules:
    /// - If `backed_by` is set, it must be `Int` or `String`.
    /// - Backed enums: every case must have a value (`case_value`).
    /// - Unit enums: no case may have a value (`case`).
    pub fn build(self) -> Result<(), PluginError> {
        let def = &self.def;

        // 1. Backing type must be Int or String if set.
        if let Some(ref bt) = def.backing_type {
            match bt {
                PhpType::Int | PhpType::String => {}
                other => {
                    return Err(PluginError::Config(format!(
                        "enum '{}': invalid backing type '{:?}' — must be Int or String",
                        def.fqn, other
                    )));
                }
            }
        }

        let is_backed = def.backing_type.is_some();

        for case in &def.cases {
            if is_backed {
                // Backed enum: all cases must have a value.
                if case.value.is_none() {
                    return Err(PluginError::Config(format!(
                        "enum '{}': backed enum case '{}' has no value — use case_value()",
                        def.fqn, case.name
                    )));
                }
            } else {
                // Unit enum: no case may have a value.
                if case.value.is_some() {
                    return Err(PluginError::Config(format!(
                        "enum '{}': unit enum case '{}' has a value — use case() for unit enums",
                        def.fqn, case.name
                    )));
                }
            }
        }

        let _ = def;
        self.target.push(self.def);
        Ok(())
    }
}

// ─── EnumMethodBuilder ────────────────────────────────────────────────────────

pub struct EnumMethodBuilder<'a> {
    parent: EnumBuilder<'a>,
    method_def: PhpMethodDef,
}

impl<'a> EnumMethodBuilder<'a> {
    /// Add a required parameter.
    pub fn param(mut self, name: &str, php_type: PhpType) -> Self {
        self.method_def
            .params
            .push(PhpParamDef::required(name, php_type));
        self
    }

    /// Add an optional parameter with a default value.
    pub fn optional_param(mut self, name: &str, php_type: PhpType, default: PhpValue) -> Self {
        self.method_def
            .params
            .push(PhpParamDef::optional(name, php_type, default));
        self
    }

    /// Add a variadic parameter. Also marks the method as variadic.
    pub fn variadic_param(mut self, name: &str, php_type: PhpType) -> Self {
        self.method_def.is_variadic = true;
        self.method_def
            .params
            .push(PhpParamDef::variadic(name, php_type));
        self
    }

    /// Set the return type.
    pub fn returns(mut self, php_type: PhpType) -> Self {
        self.method_def.return_type = Some(php_type);
        self
    }

    /// Attach a handler, push the method to the parent enum, and return the parent builder.
    pub fn handler(
        mut self,
        f: impl Fn(&mut NativeCall) -> Result<(), PhpError> + Send + Sync + 'static,
    ) -> EnumBuilder<'a> {
        self.method_def.handler = Some(Box::new(f));
        self.parent.def.methods.push(self.method_def);
        self.parent
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_enum(f: impl FnOnce(EnumBuilder<'_>)) -> PhpEnumDef {
        let mut enums = Vec::new();
        let builder = EnumBuilder::new("Test\\Status", "test_plugin", &mut enums);
        f(builder);
        assert_eq!(enums.len(), 1);
        enums.pop().unwrap()
    }

    #[test]
    fn test_unit_enum() {
        let e = collect_enum(|b| {
            b.case("Active")
                .case("Inactive")
                .case("Pending")
                .build()
                .unwrap();
        });
        assert_eq!(e.fqn, "Test\\Status");
        assert_eq!(e.plugin_name, "test_plugin");
        assert!(e.backing_type.is_none());
        assert_eq!(e.cases.len(), 3);
        assert_eq!(e.cases[0].name, "Active");
        assert!(e.cases[0].value.is_none());
        assert_eq!(e.cases[1].name, "Inactive");
        assert_eq!(e.cases[2].name, "Pending");
    }

    #[test]
    fn test_backed_enum_string() {
        let e = collect_enum(|b| {
            b.backed_by(PhpType::String)
                .case_value("Active", PhpValue::String("active".to_string()))
                .case_value("Inactive", PhpValue::String("inactive".to_string()))
                .build()
                .unwrap();
        });
        assert_eq!(e.backing_type, Some(PhpType::String));
        assert_eq!(e.cases.len(), 2);
        assert_eq!(
            e.cases[0].value,
            Some(PhpValue::String("active".to_string()))
        );
    }

    #[test]
    fn test_backed_enum_int() {
        let e = collect_enum(|b| {
            b.backed_by(PhpType::Int)
                .case_value("Low", PhpValue::Int(1))
                .case_value("High", PhpValue::Int(2))
                .build()
                .unwrap();
        });
        assert_eq!(e.backing_type, Some(PhpType::Int));
        assert_eq!(e.cases[0].value, Some(PhpValue::Int(1)));
        assert_eq!(e.cases[1].value, Some(PhpValue::Int(2)));
    }

    #[test]
    fn test_backed_enum_with_unit_case_fails() {
        let mut enums = Vec::new();
        let builder = EnumBuilder::new("Test\\Status", "test_plugin", &mut enums);
        let result = builder
            .backed_by(PhpType::String)
            .case_value("Active", PhpValue::String("active".to_string()))
            .case("Pending") // unit case in a backed enum → error
            .build();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Pending") && err.contains("no value"));
    }

    #[test]
    fn test_unit_enum_with_valued_case_fails() {
        let mut enums = Vec::new();
        let builder = EnumBuilder::new("Test\\Status", "test_plugin", &mut enums);
        let result = builder
            .case("Active")
            .case_value("Inactive", PhpValue::Int(0)) // valued case in a unit enum → error
            .build();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("Inactive") && err.contains("value"));
    }

    #[test]
    fn test_invalid_backing_type_fails() {
        let mut enums = Vec::new();
        let builder = EnumBuilder::new("Test\\Status", "test_plugin", &mut enums);
        let result = builder.backed_by(PhpType::Bool).build();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("invalid backing type"));
    }

    #[test]
    fn test_enum_implements() {
        let e = collect_enum(|b| {
            b.implements("Stringable")
                .implements("JsonSerializable")
                .build()
                .unwrap();
        });
        assert_eq!(e.interfaces.len(), 2);
        assert!(e.interfaces.contains(&"Stringable".to_string()));
        assert!(e.interfaces.contains(&"JsonSerializable".to_string()));
    }

    #[test]
    fn test_enum_method() {
        let e = collect_enum(|b| {
            b.case("Active")
                .method("label")
                .returns(PhpType::String)
                .handler(|_call| Ok(()))
                .build()
                .unwrap();
        });
        assert_eq!(e.methods.len(), 1);
        let m = &e.methods[0];
        assert_eq!(m.name, "label");
        assert_eq!(m.return_type, Some(PhpType::String));
        assert!(m.handler.is_some());
        // Enum methods are public by default.
        assert_eq!(m.visibility, Visibility::Public);
    }

    #[test]
    fn test_enum_constant() {
        let e = collect_enum(|b| {
            b.constant(
                "DEFAULT_CASE",
                PhpValue::String("active".to_string()),
                Visibility::Public,
            )
            .build()
            .unwrap();
        });
        assert_eq!(e.constants.len(), 1);
        let c = &e.constants[0];
        assert_eq!(c.name, "DEFAULT_CASE");
        assert_eq!(c.value, PhpValue::String("active".to_string()));
        assert_eq!(c.visibility, Visibility::Public);
    }
}
