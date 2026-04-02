use std::fmt;

use crate::bridge::call::NativeCall;

// ─── Native Plugin Function Trait ────────────────────────────

/// Handler for a plugin-registered PHP function using the native bridge.
/// Receives a `NativeCall` for direct zval access (zero serialization).
pub trait PluginNativeFunction: Send + Sync {
    fn handle(&self, call: &mut NativeCall) -> Result<(), PhpError>;
}

/// Closure adapter for PluginNativeFunction.
impl<F> PluginNativeFunction for F
where
    F: Fn(&mut NativeCall) -> Result<(), PhpError> + Send + Sync,
{
    fn handle(&self, call: &mut NativeCall) -> Result<(), PhpError> {
        (self)(call)
    }
}

/// Stored definition of a native plugin function.
pub struct PluginNativeFunctionDef {
    /// Full function name: `oxphp_{plugin}_{name}`
    pub name: String,
    pub plugin_name: String,
    pub params: Vec<PhpParam>,
    pub return_type: PhpType,
    pub handler: Box<dyn PluginNativeFunction>,
}

// ─── PHP Function Registration ───────────────────────────────

/// Parameter type declaration for a plugin PHP function.
#[derive(Debug, Clone)]
pub struct PhpParam {
    pub name: String,
    pub param_type: PhpType,
    pub required: bool,
    /// Display string for the default value (metadata only, not used at runtime).
    pub default: Option<String>,
}

impl PhpParam {
    pub fn required(name: &str, param_type: PhpType) -> Self {
        Self {
            name: name.to_string(),
            param_type,
            required: true,
            default: None,
        }
    }

    pub fn optional(name: &str, param_type: PhpType, default: impl fmt::Display) -> Self {
        Self {
            name: name.to_string(),
            param_type,
            required: false,
            default: Some(default.to_string()),
        }
    }
}

/// PHP type hint (maps to IS_* constants in Zend).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhpType {
    Null,
    Bool,
    Int,
    Float,
    String,
    Array,
    Object,
    Mixed,
    Void,
}

/// Error returned by plugin PHP functions.
#[derive(Debug, thiserror::Error)]
pub enum PhpError {
    #[error("Argument count: expected {expected}, got {got}")]
    ArgCount { expected: usize, got: usize },

    #[error("Type error: expected {expected}, got {got}")]
    TypeError {
        expected: &'static str,
        got: &'static str,
    },

    #[error("Function not found: {0}")]
    FunctionNotFound(String),

    #[error("Call failed: {0}")]
    CallFailed(String),

    #[error("Extension not loaded: {0}")]
    ExtensionNotLoaded(String),

    #[error("{0}")]
    Custom(String),

    #[error("PHP Exception [{class}]: {message} (code: {code})")]
    Exception {
        class: String,
        message: String,
        code: i64,
    },
}

/// Represents a PHP exception caught from `call_php()`.
#[derive(Debug, Clone)]
pub struct PhpException {
    pub class: String,
    pub message: String,
    pub code: i64,
}

impl From<PhpException> for PhpError {
    fn from(e: PhpException) -> Self {
        PhpError::Exception {
            class: e.class,
            message: e.message,
            code: e.code,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PhpParam tests ──

    #[test]
    fn test_php_param_required() {
        let p = PhpParam::required("name", PhpType::String);
        assert_eq!(p.name, "name");
        assert_eq!(p.param_type, PhpType::String);
        assert!(p.required);
        assert!(p.default.is_none());
    }

    #[test]
    fn test_php_param_optional() {
        let p = PhpParam::optional("count", PhpType::Int, 0);
        assert!(!p.required);
        assert_eq!(p.default, Some("0".to_string()));
    }

    // ── PhpError display tests ──

    #[test]
    fn test_php_error_exception_display() {
        let e = PhpError::Exception {
            class: "InvalidArgumentException".into(),
            message: "bad input".into(),
            code: 42,
        };
        assert_eq!(e.to_string(), "PHP Exception [InvalidArgumentException]: bad input (code: 42)");
    }

    #[test]
    fn test_php_exception_into_php_error() {
        let exc = PhpException {
            class: "RuntimeException".into(),
            message: "fail".into(),
            code: 1,
        };
        let err: PhpError = exc.into();
        match err {
            PhpError::Exception { class, message, code } => {
                assert_eq!(class, "RuntimeException");
                assert_eq!(message, "fail");
                assert_eq!(code, 1);
            }
            _ => panic!("Expected Exception variant"),
        }
    }

    #[test]
    fn test_php_error_display() {
        let e = PhpError::ArgCount {
            expected: 2,
            got: 1,
        };
        assert_eq!(e.to_string(), "Argument count: expected 2, got 1");

        let e = PhpError::TypeError {
            expected: "string",
            got: "int",
        };
        assert_eq!(e.to_string(), "Type error: expected string, got int");
    }
}
