use super::php::{PhpArray, PhpError, PhpObject, PhpType, PhpValue};

// ─── IntoPhpValue ────────────────────────────────────────────

/// Auto-convert Rust types to PhpValue.
/// Used by macros to reduce `PhpValue::String("...".into())` noise.
pub trait IntoPhpValue {
    fn into_php_value(self) -> PhpValue;
}

impl IntoPhpValue for PhpValue {
    fn into_php_value(self) -> PhpValue {
        self
    }
}
impl IntoPhpValue for bool {
    fn into_php_value(self) -> PhpValue {
        PhpValue::Bool(self)
    }
}
impl IntoPhpValue for i64 {
    fn into_php_value(self) -> PhpValue {
        PhpValue::Int(self)
    }
}
impl IntoPhpValue for i32 {
    fn into_php_value(self) -> PhpValue {
        PhpValue::Int(self as i64)
    }
}
impl IntoPhpValue for usize {
    fn into_php_value(self) -> PhpValue {
        PhpValue::Int(self as i64)
    }
}
impl IntoPhpValue for f64 {
    fn into_php_value(self) -> PhpValue {
        PhpValue::Float(self)
    }
}
impl IntoPhpValue for String {
    fn into_php_value(self) -> PhpValue {
        PhpValue::String(self)
    }
}
impl IntoPhpValue for &str {
    fn into_php_value(self) -> PhpValue {
        PhpValue::String(self.to_string())
    }
}
impl IntoPhpValue for PhpArray {
    fn into_php_value(self) -> PhpValue {
        PhpValue::Array(self)
    }
}
impl IntoPhpValue for PhpObject {
    fn into_php_value(self) -> PhpValue {
        PhpValue::Object(self)
    }
}

// ─── FromPhpValue ────────────────────────────────────────────

/// Extract typed value from a PhpValue reference.
pub trait FromPhpValue: Sized {
    fn from_php_value(val: &PhpValue) -> Option<Self>;
}

impl FromPhpValue for String {
    fn from_php_value(v: &PhpValue) -> Option<Self> {
        v.as_str().map(String::from)
    }
}
impl FromPhpValue for i64 {
    fn from_php_value(v: &PhpValue) -> Option<Self> {
        v.as_int()
    }
}
impl FromPhpValue for f64 {
    fn from_php_value(v: &PhpValue) -> Option<Self> {
        v.as_float()
    }
}
impl FromPhpValue for bool {
    fn from_php_value(v: &PhpValue) -> Option<Self> {
        v.as_bool()
    }
}
impl FromPhpValue for PhpArray {
    fn from_php_value(v: &PhpValue) -> Option<Self> {
        v.as_array().cloned()
    }
}
impl FromPhpValue for PhpObject {
    fn from_php_value(v: &PhpValue) -> Option<Self> {
        v.as_object().cloned()
    }
}
impl FromPhpValue for PhpValue {
    fn from_php_value(v: &PhpValue) -> Option<Self> {
        Some(v.clone())
    }
}

// ─── PhpTypeMapped ───────────────────────────────────────────

/// Resolve Rust type → PhpType at compile time.
pub trait PhpTypeMapped {
    const PHP_TYPE: PhpType;
}

impl PhpTypeMapped for String {
    const PHP_TYPE: PhpType = PhpType::String;
}
impl PhpTypeMapped for i64 {
    const PHP_TYPE: PhpType = PhpType::Int;
}
impl PhpTypeMapped for f64 {
    const PHP_TYPE: PhpType = PhpType::Float;
}
impl PhpTypeMapped for bool {
    const PHP_TYPE: PhpType = PhpType::Bool;
}
impl PhpTypeMapped for PhpArray {
    const PHP_TYPE: PhpType = PhpType::Array;
}
impl PhpTypeMapped for PhpObject {
    const PHP_TYPE: PhpType = PhpType::Object;
}
impl PhpTypeMapped for PhpValue {
    const PHP_TYPE: PhpType = PhpType::Mixed;
}
impl PhpTypeMapped for () {
    const PHP_TYPE: PhpType = PhpType::Void;
}

/// Resolve Rust type → PhpType (used by php_function! macro).
pub fn php_type_of<T: PhpTypeMapped>() -> PhpType {
    T::PHP_TYPE
}

/// Extract a typed value from args[index]. Used by php_function! and php_args!.
pub fn php_extract_arg<T: FromPhpValue>(
    args: &[PhpValue],
    index: usize,
    _name: &str,
) -> Result<T, PhpError> {
    let val = args.get(index).ok_or(PhpError::ArgCount {
        expected: index + 1,
        got: args.len(),
    })?;
    T::from_php_value(val).ok_or_else(|| PhpError::TypeError {
        expected: std::any::type_name::<T>(),
        got: val.type_name(),
    })
}

// ─── Macros ──────────────────────────────────────────────────

/// Construct a PhpValue::Array from a literal.
///
/// Dict (string keys):  `php_array! { "key" => value, "key2" => value2 }`
/// List (int keys):     `php_array! [ value1, value2, value3 ]`
#[macro_export]
macro_rules! php_array {
    // Dict: { "key" => value, ... } — string keys only
    ({ $($key:expr => $val:expr),* $(,)? }) => {
        $crate::plugin::PhpValue::Array($crate::plugin::PhpArray::from_pairs([
            $( ($key, $crate::plugin::macros::IntoPhpValue::into_php_value($val)) ),*
        ]))
    };

    // List: [ value, ... ] — sequential int keys (0, 1, 2, ...)
    ([ $($val:expr),* $(,)? ]) => {
        $crate::plugin::PhpValue::Array($crate::plugin::PhpArray::from_vec(vec![
            $( $crate::plugin::macros::IntoPhpValue::into_php_value($val) ),*
        ]))
    };
}

/// Construct a PhpValue::Object from a literal.
///
/// stdClass:    `php_object! { prop: value, prop2: value2 }`
/// Named class: `php_object! { "ClassName" => prop: value, prop2: value2 }`
#[macro_export]
macro_rules! php_object {
    // stdClass (default)
    ({ $($prop:ident : $val:expr),* $(,)? }) => {
        $crate::plugin::PhpValue::Object($crate::plugin::PhpObject::stdclass([
            $( (stringify!($prop), $crate::plugin::macros::IntoPhpValue::into_php_value($val)) ),*
        ]))
    };

    // Named class
    ({ $class:expr => $($prop:ident : $val:expr),* $(,)? }) => {
        $crate::plugin::PhpValue::Object($crate::plugin::PhpObject::new($class, [
            $( (stringify!($prop), $crate::plugin::macros::IntoPhpValue::into_php_value($val)) ),*
        ]))
    };
}

/// Call a PHP function via PhpCallContext.
///
///   `php_call!(ctx, "strtoupper", "hello")` → `Result<PhpValue, PhpError>`
///
/// Arguments auto-convert via IntoPhpValue.
#[macro_export]
macro_rules! php_call {
    ($ctx:expr, $func:expr $(, $arg:expr)* $(,)?) => {
        $ctx.call_function($func, &[ $( $crate::plugin::macros::IntoPhpValue::into_php_value($arg) ),* ])
    };
}

/// Extract typed arguments from a `&[PhpValue]` slice.
///
///   `let (name, count) = php_args!(args, name: String, count: i64)?;`
#[macro_export]
macro_rules! php_args {
    ($args:expr, $($name:ident : $ty:ty),+ $(,)?) => {{
        let mut __idx = 0usize;
        let __result: Result<($($ty,)+), $crate::plugin::PhpError> = (|| {
            $(
                let $name: $ty = $crate::plugin::macros::php_extract_arg::<$ty>($args, __idx, stringify!($name))?;
                __idx += 1;
            )+
            let _ = __idx;
            Ok(($($name,)+))
        })();
        __result
    }};
}

/// Register a PHP function on a PluginContext.
///
/// Required params only:
/// ```ignore
/// php_function!(ctx, "name", fn(param: Type) -> ReturnType { body })
/// ```
///
/// With optional params:
/// ```ignore
/// php_function!(ctx, "name", fn(req: Type; opt?: Type = default) -> ReturnType { body })
/// ```
#[macro_export]
macro_rules! php_function {
    // Required params only
    ($ctx:ident, $name:expr,
     fn($($param:ident : $ptype:ty),* $(,)?) -> $ret:ty $body:block
    ) => {
        $ctx.register_php_function(
            $name,
            vec![$( $crate::plugin::PhpParam::required(stringify!($param), $crate::plugin::macros::php_type_of::<$ptype>()) ),*],
            $crate::plugin::macros::php_type_of::<$ret>(),
            |__ctx: &$crate::plugin::PhpCallContext, __args: &[$crate::plugin::PhpValue]| -> Result<$crate::plugin::PhpValue, $crate::plugin::PhpError> {
                #[allow(unused_variables)]
                let $ctx = __ctx;
                let mut __idx = 0usize;
                $(
                    let $param: $ptype = $crate::plugin::macros::php_extract_arg::<$ptype>(__args, __idx, stringify!($param))?;
                    __idx += 1;
                )*
                let _ = __idx;
                let __result: $ret = (|$($param: $ptype),*| -> Result<$ret, $crate::plugin::PhpError> { $body })($($param),*)?;
                Ok($crate::plugin::macros::IntoPhpValue::into_php_value(__result))
            },
        );
    };

    // With optional params (? suffix)
    ($ctx:ident, $name:expr,
     fn($($req_param:ident : $req_type:ty),* $(,)?
        $(; $opt_param:ident ?: $opt_type:ty = $opt_default:expr),* $(,)?
     ) -> $ret:ty $body:block
    ) => {
        $ctx.register_php_function(
            $name,
            vec![
                $( $crate::plugin::PhpParam::required(stringify!($req_param), $crate::plugin::macros::php_type_of::<$req_type>()) ,)*
                $( $crate::plugin::PhpParam::optional(stringify!($opt_param), $crate::plugin::macros::php_type_of::<$opt_type>(), $crate::plugin::macros::IntoPhpValue::into_php_value($opt_default)) ,)*
            ],
            $crate::plugin::macros::php_type_of::<$ret>(),
            |__ctx: &$crate::plugin::PhpCallContext, __args: &[$crate::plugin::PhpValue]| -> Result<$crate::plugin::PhpValue, $crate::plugin::PhpError> {
                #[allow(unused_variables)]
                let $ctx = __ctx;
                let mut __idx = 0usize;
                $(
                    let $req_param = $crate::plugin::macros::php_extract_arg::<$req_type>(__args, __idx, stringify!($req_param))?;
                    __idx += 1;
                )*
                $(
                    let $opt_param = if __idx < __args.len() {
                        $crate::plugin::macros::php_extract_arg::<$opt_type>(__args, __idx, stringify!($opt_param))?
                    } else {
                        $opt_default
                    };
                    __idx += 1;
                )*
                let _ = __idx;
                #[allow(clippy::redundant_closure_call)]
                let __result = (|| -> Result<$ret, $crate::plugin::PhpError> { $body })()?;
                Ok($crate::plugin::macros::IntoPhpValue::into_php_value(__result))
            },
        );
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── IntoPhpValue tests ──

    #[test]
    fn test_into_php_value_primitives() {
        assert_eq!(true.into_php_value(), PhpValue::Bool(true));
        assert_eq!(42i64.into_php_value(), PhpValue::Int(42));
        assert_eq!(7i32.into_php_value(), PhpValue::Int(7));
        assert_eq!(3usize.into_php_value(), PhpValue::Int(3));
        assert_eq!(3.14f64.into_php_value(), PhpValue::Float(3.14));
        assert_eq!("hello".into_php_value(), PhpValue::String("hello".into()));
        assert_eq!(
            String::from("world").into_php_value(),
            PhpValue::String("world".into())
        );
    }

    #[test]
    fn test_into_php_value_passthrough() {
        let val = PhpValue::Int(99);
        assert_eq!(val.into_php_value(), PhpValue::Int(99));
    }

    #[test]
    fn test_into_php_value_array() {
        let arr = PhpArray::from_vec(vec![PhpValue::Int(1)]);
        let val = arr.into_php_value();
        assert!(val.as_array().is_some());
    }

    #[test]
    fn test_into_php_value_object() {
        let obj = PhpObject::stdclass([("x", PhpValue::Int(1))]);
        let val = obj.into_php_value();
        assert!(val.as_object().is_some());
    }

    // ── FromPhpValue tests ──

    #[test]
    fn test_from_php_value_string() {
        let val = PhpValue::String("hello".into());
        assert_eq!(String::from_php_value(&val), Some("hello".to_string()));
        assert_eq!(String::from_php_value(&PhpValue::Int(1)), None);
    }

    #[test]
    fn test_from_php_value_i64() {
        assert_eq!(i64::from_php_value(&PhpValue::Int(42)), Some(42));
        assert_eq!(i64::from_php_value(&PhpValue::Float(1.0)), None);
    }

    #[test]
    fn test_from_php_value_f64() {
        assert_eq!(f64::from_php_value(&PhpValue::Float(3.14)), Some(3.14));
        assert_eq!(f64::from_php_value(&PhpValue::Int(1)), None);
    }

    #[test]
    fn test_from_php_value_bool() {
        assert_eq!(bool::from_php_value(&PhpValue::Bool(true)), Some(true));
        assert_eq!(bool::from_php_value(&PhpValue::Null), None);
    }

    #[test]
    fn test_from_php_value_php_value() {
        let val = PhpValue::Null;
        assert_eq!(PhpValue::from_php_value(&val), Some(PhpValue::Null));
    }

    // ── PhpTypeMapped tests ──

    #[test]
    fn test_php_type_of() {
        assert_eq!(php_type_of::<String>(), PhpType::String);
        assert_eq!(php_type_of::<i64>(), PhpType::Int);
        assert_eq!(php_type_of::<f64>(), PhpType::Float);
        assert_eq!(php_type_of::<bool>(), PhpType::Bool);
        assert_eq!(php_type_of::<PhpArray>(), PhpType::Array);
        assert_eq!(php_type_of::<PhpObject>(), PhpType::Object);
        assert_eq!(php_type_of::<PhpValue>(), PhpType::Mixed);
        assert_eq!(php_type_of::<()>(), PhpType::Void);
    }

    // ── php_extract_arg tests ──

    #[test]
    fn test_php_extract_arg_success() {
        let args = vec![PhpValue::String("hello".into()), PhpValue::Int(42)];
        let s: String = php_extract_arg(&args, 0, "name").unwrap();
        assert_eq!(s, "hello");
        let n: i64 = php_extract_arg(&args, 1, "count").unwrap();
        assert_eq!(n, 42);
    }

    #[test]
    fn test_php_extract_arg_out_of_bounds() {
        let args = vec![PhpValue::Int(1)];
        let result = php_extract_arg::<String>(&args, 5, "missing");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Argument count"));
    }

    #[test]
    fn test_php_extract_arg_type_mismatch() {
        let args = vec![PhpValue::Int(1)];
        let result = php_extract_arg::<String>(&args, 0, "val");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Type error"));
    }

    // ── php_array! macro tests ──

    #[test]
    fn test_php_array_dict_macro() {
        let val = php_array!({ "name" => "John", "age" => 30i64 });
        let arr = val.as_array().unwrap();
        assert_eq!(arr.get("name"), Some(&PhpValue::String("John".into())));
        assert_eq!(arr.get("age"), Some(&PhpValue::Int(30)));
    }

    #[test]
    fn test_php_array_list_macro() {
        let val = php_array!([1i64, 2i64, 3i64]);
        let arr = val.as_array().unwrap();
        assert_eq!(arr.len(), 3);
        assert!(arr.is_list());
        assert_eq!(arr.get_index(0), Some(&PhpValue::Int(1)));
    }

    #[test]
    fn test_php_array_empty_list() {
        let val: PhpValue = php_array!([]);
        assert!(val.as_array().unwrap().is_empty());
    }

    // ── php_object! macro tests ──

    #[test]
    fn test_php_object_stdclass_macro() {
        let val = php_object!({ name: "test", active: true });
        let obj = val.as_object().unwrap();
        assert_eq!(obj.class_name, "stdClass");
        assert_eq!(obj.get("name"), Some(&PhpValue::String("test".into())));
        assert_eq!(obj.get("active"), Some(&PhpValue::Bool(true)));
    }

    #[test]
    fn test_php_object_named_class_macro() {
        let val = php_object!({ "MyClass" => x: 1i64, y: 2i64 });
        let obj = val.as_object().unwrap();
        assert_eq!(obj.class_name, "MyClass");
        assert_eq!(obj.get("x"), Some(&PhpValue::Int(1)));
    }

    // ── php_args! macro tests ──

    #[test]
    fn test_php_args_macro() {
        let args = vec![PhpValue::String("hello".into()), PhpValue::Int(42)];
        let (name, count) = php_args!(&args, name: String, count: i64).unwrap();
        assert_eq!(name, "hello");
        assert_eq!(count, 42);
    }

    #[test]
    fn test_php_args_macro_type_error() {
        let args = vec![PhpValue::Int(1)];
        let result = php_args!(&args, val: String);
        assert!(result.is_err());
    }

    #[test]
    fn test_php_args_macro_missing() {
        let args: Vec<PhpValue> = vec![];
        let result = php_args!(&args, val: String);
        assert!(result.is_err());
    }
}
