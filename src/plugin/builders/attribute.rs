use crate::plugin::types::*;
use crate::plugin::PluginError;

use super::definitions::{PhpAttributeDef, PhpParamDef, PhpPropertyDef};

// ─── Target Constants ─────────────────────────────────────────────────────────

pub const ATTR_TARGET_CLASS: u32 = 0x01;
pub const ATTR_TARGET_FUNCTION: u32 = 0x02;
pub const ATTR_TARGET_METHOD: u32 = 0x04;
pub const ATTR_TARGET_PROPERTY: u32 = 0x08;
pub const ATTR_TARGET_PARAMETER: u32 = 0x10;
pub const ATTR_TARGET_CONSTANT: u32 = 0x20;
pub const ATTR_TARGET_ALL: u32 = 0x3F;

// ─── AttributeBuilder ────────────────────────────────────────────────────────

pub struct AttributeBuilder<'a> {
    def: PhpAttributeDef,
    target: &'a mut Vec<PhpAttributeDef>,
}

impl<'a> AttributeBuilder<'a> {
    pub(crate) fn new(fqn: &str, plugin_name: &str, target: &'a mut Vec<PhpAttributeDef>) -> Self {
        let mut def = PhpAttributeDef::new(fqn);
        def.plugin_name = plugin_name.to_string();
        Self { def, target }
    }

    /// Override the valid target bitmask (default: `ATTR_TARGET_ALL`).
    pub fn target(mut self, targets: u32) -> Self {
        self.def.targets = targets;
        self
    }

    /// Mark this attribute as repeatable (can appear multiple times on the same declaration).
    pub fn repeatable(mut self) -> Self {
        self.def.repeatable = true;
        self
    }

    /// Add a required constructor parameter.
    pub fn param(mut self, name: &str, php_type: PhpType) -> Self {
        self.def.params.push(PhpParamDef::required(name, php_type));
        self
    }

    /// Add an optional constructor parameter with a default value.
    pub fn optional_param(mut self, name: &str, php_type: PhpType, default: PhpValue) -> Self {
        self.def
            .params
            .push(PhpParamDef::optional(name, php_type, default));
        self
    }

    /// Add a promoted property (used in attribute constructor promotion).
    pub fn property(mut self, name: &str, php_type: PhpType, visibility: Visibility) -> Self {
        self.def.properties.push(PhpPropertyDef {
            name: name.to_string(),
            php_type,
            visibility,
            modifiers: Modifiers::empty(),
            default: None,
        });
        self
    }

    /// Push the attribute definition to the target collection.
    pub fn build(self) -> Result<(), PluginError> {
        self.target.push(self.def);
        Ok(())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_attribute(f: impl FnOnce(AttributeBuilder<'_>)) -> PhpAttributeDef {
        let mut attributes = Vec::new();
        let builder = AttributeBuilder::new("Test\\Route", "test_plugin", &mut attributes);
        f(builder);
        assert_eq!(attributes.len(), 1);
        attributes.pop().unwrap()
    }

    #[test]
    fn test_minimal_attribute() {
        let attr = collect_attribute(|b| {
            b.build().unwrap();
        });
        assert_eq!(attr.fqn, "Test\\Route");
        assert_eq!(attr.plugin_name, "test_plugin");
        assert_eq!(attr.targets, ATTR_TARGET_ALL);
        assert!(!attr.repeatable);
        assert!(attr.params.is_empty());
        assert!(attr.properties.is_empty());
    }

    #[test]
    fn test_attribute_targets() {
        let attr = collect_attribute(|b| {
            b.target(ATTR_TARGET_CLASS | ATTR_TARGET_METHOD)
                .build()
                .unwrap();
        });
        assert_eq!(attr.targets, ATTR_TARGET_CLASS | ATTR_TARGET_METHOD);
        assert_ne!(attr.targets, ATTR_TARGET_ALL);

        // Verify the individual bits.
        assert_ne!(attr.targets & ATTR_TARGET_CLASS, 0);
        assert_ne!(attr.targets & ATTR_TARGET_METHOD, 0);
        assert_eq!(attr.targets & ATTR_TARGET_FUNCTION, 0);
        assert_eq!(attr.targets & ATTR_TARGET_PROPERTY, 0);
        assert_eq!(attr.targets & ATTR_TARGET_PARAMETER, 0);
        assert_eq!(attr.targets & ATTR_TARGET_CONSTANT, 0);
    }

    #[test]
    fn test_attribute_repeatable() {
        let attr = collect_attribute(|b| {
            b.repeatable().build().unwrap();
        });
        assert!(attr.repeatable);
    }

    #[test]
    fn test_attribute_params() {
        let attr = collect_attribute(|b| {
            b.param("path", PhpType::String)
                .optional_param("methods", PhpType::Array, PhpValue::Array)
                .build()
                .unwrap();
        });
        assert_eq!(attr.params.len(), 2);
        let p0 = &attr.params[0];
        assert_eq!(p0.name, "path");
        assert_eq!(p0.php_type, PhpType::String);
        assert!(p0.required);

        let p1 = &attr.params[1];
        assert_eq!(p1.name, "methods");
        assert_eq!(p1.php_type, PhpType::Array);
        assert!(!p1.required);
        assert_eq!(p1.default, Some(PhpValue::Array));
    }

    #[test]
    fn test_attribute_properties() {
        let attr = collect_attribute(|b| {
            b.property("path", PhpType::String, Visibility::Public)
                .property("methods", PhpType::Array, Visibility::Protected)
                .build()
                .unwrap();
        });
        assert_eq!(attr.properties.len(), 2);
        let p0 = &attr.properties[0];
        assert_eq!(p0.name, "path");
        assert_eq!(p0.php_type, PhpType::String);
        assert_eq!(p0.visibility, Visibility::Public);

        let p1 = &attr.properties[1];
        assert_eq!(p1.name, "methods");
        assert_eq!(p1.visibility, Visibility::Protected);
    }
}
