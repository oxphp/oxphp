use crate::bridge::call::NativeCall;
use crate::plugin::php::{PhpError, PluginNativeFunction};
use crate::plugin::types::*;
use crate::plugin::PluginError;

use super::definitions::{PhpFunctionDef, PhpParamDef};

// ─── FunctionBuilder ──────────────────────────────────────────────────────────

pub struct FunctionBuilder<'a> {
    def: PhpFunctionDef,
    target: &'a mut Vec<PhpFunctionDef>,
}

impl<'a> FunctionBuilder<'a> {
    pub(crate) fn new(fqn: &str, plugin_name: &str, target: &'a mut Vec<PhpFunctionDef>) -> Self {
        let mut def = PhpFunctionDef::new(fqn);
        def.plugin_name = plugin_name.to_string();
        Self { def, target }
    }

    /// Add a required parameter.
    pub fn param(mut self, name: &str, php_type: PhpType) -> Self {
        self.def.params.push(PhpParamDef::required(name, php_type));
        self
    }

    /// Add an optional parameter with a default value.
    pub fn optional_param(mut self, name: &str, php_type: PhpType, default: PhpValue) -> Self {
        self.def.params.push(PhpParamDef::optional(name, php_type, default));
        self
    }

    /// Add a variadic parameter. Also marks the function as variadic.
    pub fn variadic_param(mut self, name: &str, php_type: PhpType) -> Self {
        self.def.is_variadic = true;
        self.def.params.push(PhpParamDef::variadic(name, php_type));
        self
    }

    /// Set the return type.
    pub fn returns(mut self, php_type: PhpType) -> Self {
        self.def.return_type = Some(php_type);
        self
    }

    /// Attach a handler AND push the function definition to the target collection.
    /// This is a terminal operation — returns `Ok(())` on success.
    pub fn handler(
        mut self,
        f: impl Fn(&mut NativeCall) -> Result<(), PhpError> + Send + Sync + 'static,
    ) -> Result<(), PluginError> {
        self.def.handler = Some(Box::new(f) as Box<dyn PluginNativeFunction>);
        self.target.push(self.def);
        Ok(())
    }

    /// Validate and push the function definition without setting a handler.
    ///
    /// Returns `Err` if no handler has been set (e.g. via a pre-built `Box<dyn PluginNativeFunction>`
    /// stored in `self.def.handler`). In the typical workflow, use `handler()` instead.
    pub fn build(self) -> Result<(), PluginError> {
        if self.def.handler.is_none() {
            return Err(PluginError::Config(format!(
                "function '{}': no handler set — use handler() or set def.handler before build()",
                self.def.fqn
            )));
        }
        self.target.push(self.def);
        Ok(())
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn collect_function(f: impl FnOnce(FunctionBuilder<'_>) -> Result<(), PluginError>) -> PhpFunctionDef {
        let mut functions = Vec::new();
        let builder = FunctionBuilder::new("oxphp_test_hello", "test_plugin", &mut functions);
        f(builder).unwrap();
        assert_eq!(functions.len(), 1);
        functions.pop().unwrap()
    }

    #[test]
    fn test_function_builder_basic() {
        let f = collect_function(|b| {
            b.param("name", PhpType::String)
                .returns(PhpType::String)
                .handler(|_call| Ok(()))
        });
        assert_eq!(f.fqn, "oxphp_test_hello");
        assert_eq!(f.plugin_name, "test_plugin");
        assert_eq!(f.params.len(), 1);
        assert_eq!(f.params[0].name, "name");
        assert!(f.params[0].required);
        assert_eq!(f.return_type, Some(PhpType::String));
        assert!(f.handler.is_some());
        assert!(!f.is_variadic);
        assert_eq!(f.required_params(), 1);
        assert_eq!(f.total_params(), 1);
    }

    #[test]
    fn test_function_builder_optional_and_variadic() {
        let f = collect_function(|b| {
            b.param("required_arg", PhpType::Int)
                .optional_param("optional_arg", PhpType::String, PhpValue::Null)
                .variadic_param("rest", PhpType::Mixed)
                .returns(PhpType::Void)
                .handler(|_call| Ok(()))
        });
        assert_eq!(f.params.len(), 3);
        assert!(f.params[0].required);
        assert!(!f.params[1].required);
        assert!(f.params[2].is_variadic);
        assert!(f.is_variadic);
        assert_eq!(f.required_params(), 1);
        assert_eq!(f.total_params(), 3);
    }

    #[test]
    fn test_function_builder_no_handler_fails() {
        let mut functions = Vec::new();
        let builder = FunctionBuilder::new("oxphp_test_no_handler", "test_plugin", &mut functions);
        let result = builder.param("x", PhpType::Int).build();
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("no handler"));
        // Nothing should have been pushed.
        assert!(functions.is_empty());
    }

    #[test]
    fn test_global_namespace_function() {
        // Functions without a namespace prefix are valid.
        let f = collect_function(|b| {
            b.returns(PhpType::String).handler(|_call| Ok(()))
        });
        assert_eq!(f.fqn, "oxphp_test_hello");
        assert!(f.params.is_empty());
        assert_eq!(f.return_type, Some(PhpType::String));
    }
}
