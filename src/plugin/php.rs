use std::fmt;

// ─── PHP Value Types ─────────────────────────────────────────

/// Rust representation of a PHP value (maps to zval).
/// Conversion to/from zval happens at the bridge boundary.
#[derive(Debug, Clone, PartialEq)]
pub enum PhpValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array(PhpArray),
    Object(PhpObject),
}

impl PhpValue {
    pub fn type_name(&self) -> &'static str {
        match self {
            PhpValue::Null => "null",
            PhpValue::Bool(_) => "bool",
            PhpValue::Int(_) => "int",
            PhpValue::Float(_) => "float",
            PhpValue::String(_) => "string",
            PhpValue::Array(_) => "array",
            PhpValue::Object(_) => "object",
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            PhpValue::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            PhpValue::Int(n) => Some(*n),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            PhpValue::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            PhpValue::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&PhpArray> {
        match self {
            PhpValue::Array(a) => Some(a),
            _ => None,
        }
    }

    pub fn as_object(&self) -> Option<&PhpObject> {
        match self {
            PhpValue::Object(o) => Some(o),
            _ => None,
        }
    }

    pub fn is_null(&self) -> bool {
        matches!(self, PhpValue::Null)
    }
}

impl fmt::Display for PhpValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PhpValue::Null => write!(f, "null"),
            PhpValue::Bool(b) => write!(f, "{b}"),
            PhpValue::Int(n) => write!(f, "{n}"),
            PhpValue::Float(v) => write!(f, "{v}"),
            PhpValue::String(s) => write!(f, "{s}"),
            PhpValue::Array(a) => write!(f, "array({})", a.len()),
            PhpValue::Object(o) => write!(f, "object({})", o.class_name),
        }
    }
}

// ─── PhpArrayKey ─────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum PhpArrayKey {
    Int(i64),
    String(String),
}

impl PhpArrayKey {
    pub fn to_php_value(&self) -> PhpValue {
        match self {
            PhpArrayKey::Int(n) => PhpValue::Int(*n),
            PhpArrayKey::String(s) => PhpValue::String(s.clone()),
        }
    }
}

// ─── PhpArray ────────────────────────────────────────────────

/// PHP array — ordered map with int or string keys.
#[derive(Debug, Clone)]
pub struct PhpArray {
    entries: Vec<(PhpArrayKey, PhpValue)>,
    next_int_key: i64,
}

impl PartialEq for PhpArray {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

impl PhpArray {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_int_key: 0,
        }
    }

    /// Create from key-value pairs (string keys).
    pub fn from_pairs<'a>(pairs: impl IntoIterator<Item = (&'a str, PhpValue)>) -> Self {
        Self {
            entries: pairs
                .into_iter()
                .map(|(k, v)| (PhpArrayKey::String(k.to_string()), v))
                .collect(),
            next_int_key: 0,
        }
    }

    /// Create a list (sequential int keys starting at 0).
    pub fn from_vec(values: Vec<PhpValue>) -> Self {
        let len = values.len() as i64;
        Self {
            entries: values
                .into_iter()
                .enumerate()
                .map(|(i, v)| (PhpArrayKey::Int(i as i64), v))
                .collect(),
            next_int_key: len,
        }
    }

    /// Append a value with the next sequential int key (like PHP `$a[] = val`).
    pub fn push(&mut self, value: PhpValue) {
        let key = self.next_int_key;
        self.next_int_key = key + 1;
        self.entries.push((PhpArrayKey::Int(key), value));
    }

    /// Insert or replace a string-keyed entry (like PHP `$a["key"] = val`).
    /// If the key already exists, the value is replaced in-place (PHP semantics).
    pub fn insert(&mut self, key: impl Into<String>, value: PhpValue) {
        let key = key.into();
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|(k, _)| matches!(k, PhpArrayKey::String(s) if *s == key))
        {
            entry.1 = value;
        } else {
            self.entries.push((PhpArrayKey::String(key), value));
        }
    }

    pub fn get(&self, key: &str) -> Option<&PhpValue> {
        self.entries
            .iter()
            .find(|(k, _)| matches!(k, PhpArrayKey::String(s) if s == key))
            .map(|(_, v)| v)
    }

    pub fn get_index(&self, index: i64) -> Option<&PhpValue> {
        self.entries
            .iter()
            .find(|(k, _)| matches!(k, PhpArrayKey::Int(n) if *n == index))
            .map(|(_, v)| v)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &PhpArrayKey> {
        self.entries.iter().map(|(k, _)| k)
    }

    pub fn values(&self) -> impl Iterator<Item = &PhpValue> {
        self.entries.iter().map(|(_, v)| v)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&PhpArrayKey, &PhpValue)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }

    /// True if all keys are sequential ints starting at 0 (PHP's array_is_list).
    pub fn is_list(&self) -> bool {
        self.entries
            .iter()
            .enumerate()
            .all(|(i, (k, _))| matches!(k, PhpArrayKey::Int(n) if *n == i as i64))
    }
}

impl Default for PhpArray {
    fn default() -> Self {
        Self::new()
    }
}

// ─── PhpObject ───────────────────────────────────────────────

/// PHP object — class name + property map.
#[derive(Debug, Clone, PartialEq)]
pub struct PhpObject {
    pub class_name: String,
    properties: Vec<(String, PhpValue)>,
}

impl PhpObject {
    pub fn new<'a>(class: &str, props: impl IntoIterator<Item = (&'a str, PhpValue)>) -> Self {
        Self {
            class_name: class.to_string(),
            properties: props.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
        }
    }

    pub fn stdclass<'a>(props: impl IntoIterator<Item = (&'a str, PhpValue)>) -> Self {
        Self::new("stdClass", props)
    }

    pub fn get(&self, prop: &str) -> Option<&PhpValue> {
        self.properties
            .iter()
            .find(|(k, _)| k == prop)
            .map(|(_, v)| v)
    }

    pub fn set(&mut self, prop: impl Into<String>, value: PhpValue) {
        let prop = prop.into();
        if let Some(entry) = self.properties.iter_mut().find(|(k, _)| *k == prop) {
            entry.1 = value;
        } else {
            self.properties.push((prop, value));
        }
    }

    pub fn properties(&self) -> impl Iterator<Item = (&str, &PhpValue)> {
        self.properties.iter().map(|(k, v)| (k.as_str(), v))
    }
}

// ─── PHP Function Registration ───────────────────────────────

/// Parameter type declaration for a plugin PHP function.
#[derive(Debug, Clone)]
pub struct PhpParam {
    pub name: String,
    pub param_type: PhpType,
    pub required: bool,
    pub default: Option<PhpValue>,
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

    pub fn optional(name: &str, param_type: PhpType, default: PhpValue) -> Self {
        Self {
            name: name.to_string(),
            param_type,
            required: false,
            default: Some(default),
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
}

/// Trait for plugin-registered PHP function handlers.
/// Runs on the PHP worker thread — safe to call back into PHP via PhpCallContext.
pub trait PluginPhpFunction: Send + Sync {
    fn handle(&self, ctx: &PhpCallContext, args: &[PhpValue]) -> Result<PhpValue, PhpError>;
}

/// Closure adapter for PluginPhpFunction.
impl<F> PluginPhpFunction for F
where
    F: Fn(&PhpCallContext, &[PhpValue]) -> Result<PhpValue, PhpError> + Send + Sync,
{
    fn handle(&self, ctx: &PhpCallContext, args: &[PhpValue]) -> Result<PhpValue, PhpError> {
        (self)(ctx, args)
    }
}

/// Stored definition of a plugin-registered PHP function.
/// Fields are pub for the SAPI bridge to register these as real PHP functions
/// via `zend_register_functions` (wired in Phase 8).
#[allow(dead_code)]
pub struct PluginPhpFunctionDef {
    /// Full function name: `oxphp_{plugin}_{name}`
    pub name: String,
    pub plugin_name: String,
    pub params: Vec<PhpParam>,
    pub return_type: PhpType,
    pub handler: Box<dyn PluginPhpFunction>,
}

// ─── PHP Call Context ────────────────────────────────────────

/// Context available inside a plugin PHP function handler.
/// Wraps the Zend execution state on the current PHP worker thread.
///
/// Only valid during the handler invocation — do not store or send to other threads.
pub struct PhpCallContext {
    _private: (),
}

impl PhpCallContext {
    /// Create a new PhpCallContext (internal use only).
    #[allow(dead_code)]
    pub(crate) fn new() -> Self {
        Self { _private: () }
    }

    /// Call an existing PHP function by name with arguments.
    /// TODO(phase-8): wire to `zend_call_function()` / `call_user_function()` via bridge FFI.
    /// Currently returns an error — the SAPI bridge doesn't support outbound PHP calls yet.
    pub fn call_function(&self, name: &str, _args: &[PhpValue]) -> Result<PhpValue, PhpError> {
        Err(PhpError::CallFailed(format!(
            "call_function not yet implemented for {name}"
        )))
    }

    /// Create a PHP array from Rust data.
    pub fn make_array<'a>(&self, pairs: impl IntoIterator<Item = (&'a str, PhpValue)>) -> PhpValue {
        PhpValue::Array(PhpArray::from_pairs(pairs))
    }

    /// Create a stdClass object from Rust data.
    pub fn make_object<'a>(
        &self,
        props: impl IntoIterator<Item = (&'a str, PhpValue)>,
    ) -> PhpValue {
        PhpValue::Object(PhpObject::stdclass(props))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── PhpValue tests ──

    #[test]
    fn test_php_value_type_name() {
        assert_eq!(PhpValue::Null.type_name(), "null");
        assert_eq!(PhpValue::Bool(true).type_name(), "bool");
        assert_eq!(PhpValue::Int(42).type_name(), "int");
        assert_eq!(PhpValue::Float(3.14).type_name(), "float");
        assert_eq!(PhpValue::String("hi".into()).type_name(), "string");
        assert_eq!(PhpValue::Array(PhpArray::new()).type_name(), "array");
        assert_eq!(
            PhpValue::Object(PhpObject::stdclass([])).type_name(),
            "object"
        );
    }

    #[test]
    fn test_php_value_accessors() {
        assert_eq!(PhpValue::String("hello".into()).as_str(), Some("hello"));
        assert_eq!(PhpValue::Int(42).as_str(), None);

        assert_eq!(PhpValue::Int(42).as_int(), Some(42));
        assert_eq!(PhpValue::String("x".into()).as_int(), None);

        assert_eq!(PhpValue::Float(3.14).as_float(), Some(3.14));
        assert_eq!(PhpValue::Int(1).as_float(), None);

        assert_eq!(PhpValue::Bool(true).as_bool(), Some(true));
        assert_eq!(PhpValue::Null.as_bool(), None);

        assert!(PhpValue::Null.is_null());
        assert!(!PhpValue::Int(0).is_null());
    }

    #[test]
    fn test_php_value_as_array() {
        let arr = PhpArray::from_vec(vec![PhpValue::Int(1)]);
        let val = PhpValue::Array(arr.clone());
        assert_eq!(val.as_array(), Some(&arr));
        assert_eq!(PhpValue::Null.as_array(), None);
    }

    #[test]
    fn test_php_value_as_object() {
        let obj = PhpObject::stdclass([("x", PhpValue::Int(1))]);
        let val = PhpValue::Object(obj.clone());
        assert_eq!(val.as_object(), Some(&obj));
        assert_eq!(PhpValue::Null.as_object(), None);
    }

    #[test]
    fn test_php_value_display() {
        assert_eq!(format!("{}", PhpValue::Null), "null");
        assert_eq!(format!("{}", PhpValue::Bool(true)), "true");
        assert_eq!(format!("{}", PhpValue::Int(42)), "42");
        assert_eq!(format!("{}", PhpValue::String("hi".into())), "hi");
    }

    // ── PhpArrayKey tests ──

    #[test]
    fn test_php_array_key_to_value() {
        assert_eq!(PhpArrayKey::Int(5).to_php_value(), PhpValue::Int(5));
        assert_eq!(
            PhpArrayKey::String("k".into()).to_php_value(),
            PhpValue::String("k".into())
        );
    }

    // ── PhpArray tests ──

    #[test]
    fn test_php_array_new_empty() {
        let arr = PhpArray::new();
        assert!(arr.is_empty());
        assert_eq!(arr.len(), 0);
    }

    #[test]
    fn test_php_array_from_pairs() {
        let arr = PhpArray::from_pairs([
            ("name", PhpValue::String("John".into())),
            ("age", PhpValue::Int(30)),
        ]);
        assert_eq!(arr.len(), 2);
        assert_eq!(arr.get("name"), Some(&PhpValue::String("John".into())));
        assert_eq!(arr.get("age"), Some(&PhpValue::Int(30)));
        assert_eq!(arr.get("missing"), None);
    }

    #[test]
    fn test_php_array_from_vec() {
        let arr = PhpArray::from_vec(vec![
            PhpValue::Int(10),
            PhpValue::Int(20),
            PhpValue::Int(30),
        ]);
        assert_eq!(arr.len(), 3);
        assert!(arr.is_list());
        assert_eq!(arr.get_index(0), Some(&PhpValue::Int(10)));
        assert_eq!(arr.get_index(1), Some(&PhpValue::Int(20)));
        assert_eq!(arr.get_index(2), Some(&PhpValue::Int(30)));
        assert_eq!(arr.get_index(3), None);
    }

    #[test]
    fn test_php_array_push() {
        let mut arr = PhpArray::new();
        arr.push(PhpValue::String("a".into()));
        arr.push(PhpValue::String("b".into()));
        assert_eq!(arr.len(), 2);
        assert!(arr.is_list());
        assert_eq!(arr.get_index(0), Some(&PhpValue::String("a".into())));
        assert_eq!(arr.get_index(1), Some(&PhpValue::String("b".into())));
    }

    #[test]
    fn test_php_array_insert() {
        let mut arr = PhpArray::new();
        arr.insert("key1", PhpValue::Int(1));
        arr.insert("key2", PhpValue::Int(2));
        assert_eq!(arr.get("key1"), Some(&PhpValue::Int(1)));
        assert!(!arr.is_list());
    }

    #[test]
    fn test_php_array_insert_replaces_duplicate() {
        let mut arr = PhpArray::new();
        arr.insert("key", PhpValue::Int(1));
        arr.insert("key", PhpValue::Int(2));
        // PHP semantics: last value wins, no duplicate entry
        assert_eq!(arr.get("key"), Some(&PhpValue::Int(2)));
        assert_eq!(arr.len(), 1);
    }

    #[test]
    fn test_php_array_is_list() {
        let list = PhpArray::from_vec(vec![PhpValue::Int(1), PhpValue::Int(2)]);
        assert!(list.is_list());

        let dict = PhpArray::from_pairs([("a", PhpValue::Int(1))]);
        assert!(!dict.is_list());

        let empty = PhpArray::new();
        assert!(empty.is_list());
    }

    #[test]
    fn test_php_array_keys_values_iter() {
        let arr = PhpArray::from_pairs([("a", PhpValue::Int(1)), ("b", PhpValue::Int(2))]);
        let keys: Vec<_> = arr.keys().collect();
        assert_eq!(keys.len(), 2);
        let values: Vec<_> = arr.values().collect();
        assert_eq!(values, vec![&PhpValue::Int(1), &PhpValue::Int(2)]);
        let pairs: Vec<_> = arr.iter().collect();
        assert_eq!(pairs.len(), 2);
    }

    // ── PhpObject tests ──

    #[test]
    fn test_php_object_new() {
        let obj = PhpObject::new("MyClass", [("x", PhpValue::Int(1))]);
        assert_eq!(obj.class_name, "MyClass");
        assert_eq!(obj.get("x"), Some(&PhpValue::Int(1)));
        assert_eq!(obj.get("y"), None);
    }

    #[test]
    fn test_php_object_stdclass() {
        let obj = PhpObject::stdclass([("name", PhpValue::String("test".into()))]);
        assert_eq!(obj.class_name, "stdClass");
        assert_eq!(obj.get("name"), Some(&PhpValue::String("test".into())));
    }

    #[test]
    fn test_php_object_set_existing() {
        let mut obj = PhpObject::stdclass([("x", PhpValue::Int(1))]);
        obj.set("x", PhpValue::Int(2));
        assert_eq!(obj.get("x"), Some(&PhpValue::Int(2)));
        // Should not add a duplicate entry
        assert_eq!(obj.properties().count(), 1);
    }

    #[test]
    fn test_php_object_set_new() {
        let mut obj = PhpObject::stdclass([]);
        obj.set("y", PhpValue::Bool(true));
        assert_eq!(obj.get("y"), Some(&PhpValue::Bool(true)));
    }

    #[test]
    fn test_php_object_properties() {
        let obj = PhpObject::stdclass([("a", PhpValue::Int(1)), ("b", PhpValue::Int(2))]);
        let props: Vec<_> = obj.properties().collect();
        assert_eq!(props.len(), 2);
        assert_eq!(props[0], ("a", &PhpValue::Int(1)));
        assert_eq!(props[1], ("b", &PhpValue::Int(2)));
    }

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
        let p = PhpParam::optional("count", PhpType::Int, PhpValue::Int(0));
        assert!(!p.required);
        assert_eq!(p.default, Some(PhpValue::Int(0)));
    }

    // ── PhpCallContext tests ──

    #[test]
    fn test_php_call_context_make_array() {
        let ctx = PhpCallContext::new();
        let val = ctx.make_array([("key", PhpValue::Int(1))]);
        assert!(val.as_array().is_some());
        assert_eq!(val.as_array().unwrap().get("key"), Some(&PhpValue::Int(1)));
    }

    #[test]
    fn test_php_call_context_make_object() {
        let ctx = PhpCallContext::new();
        let val = ctx.make_object([("prop", PhpValue::Bool(true))]);
        let obj = val.as_object().unwrap();
        assert_eq!(obj.class_name, "stdClass");
        assert_eq!(obj.get("prop"), Some(&PhpValue::Bool(true)));
    }

    // ── PhpError display tests ──

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

    // ── PluginPhpFunction closure adapter ──

    #[test]
    fn test_php_function_closure_adapter() {
        let handler = |_ctx: &PhpCallContext, args: &[PhpValue]| -> Result<PhpValue, PhpError> {
            Ok(args.first().cloned().unwrap_or(PhpValue::Null))
        };

        let ctx = PhpCallContext::new();
        let result = handler.handle(&ctx, &[PhpValue::Int(42)]);
        assert_eq!(result.unwrap(), PhpValue::Int(42));
    }
}
