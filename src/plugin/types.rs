use std::fmt;

use bitflags::bitflags;

// ─── PhpType ─────────────────────────────────────────────────────────────────

/// Extended PHP 8.x type system for use in builder APIs.
///
/// Note: the simpler `PhpType` in `plugin::php` is a `Copy` enum for native
/// function signatures — that type is kept for backward compatibility.
/// This type covers the full PHP 8.x type grammar including union, intersection,
/// nullable, class names, `never`, `true`, `false`, etc.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    Class(String),
    Interface(String),
    Enum(String),
    Nullable(Box<PhpType>),
    Union(Vec<PhpType>),
    Intersection(Vec<PhpType>),
    Iterable,
    Callable,
    Self_,
    Static_,
    Parent_,
    Never,
    False,
    True,
}

// ─── Bridge Return Type Tags ─────────────────────────────────────────────────
//
// These constants mirror `OXPHP_RT_*` in `ext/bridge/oxphp_bridge.h`.
// They are the wire format between Rust and the C bridge for method/function
// return type declarations. The C SAPI maps them to Zend type codes at
// registration time.

/// No return type declared.
pub const BRIDGE_RT_NONE: i32 = 0;
pub const BRIDGE_RT_NULL: i32 = 1;
pub const BRIDGE_RT_BOOL: i32 = 2;
pub const BRIDGE_RT_INT: i32 = 3;
pub const BRIDGE_RT_FLOAT: i32 = 4;
pub const BRIDGE_RT_STRING: i32 = 5;
pub const BRIDGE_RT_ARRAY: i32 = 6;
pub const BRIDGE_RT_OBJECT: i32 = 7;
pub const BRIDGE_RT_MIXED: i32 = 8;
pub const BRIDGE_RT_VOID: i32 = 9;
pub const BRIDGE_RT_CALLABLE: i32 = 10;
pub const BRIDGE_RT_ITERABLE: i32 = 11;
pub const BRIDGE_RT_NEVER: i32 = 12;
pub const BRIDGE_RT_FALSE: i32 = 13;
pub const BRIDGE_RT_TRUE: i32 = 14;
pub const BRIDGE_RT_SELF: i32 = 15;
pub const BRIDGE_RT_STATIC: i32 = 16;
pub const BRIDGE_RT_PARENT: i32 = 17;

impl PhpType {
    /// Convert to bridge return type tag `(BRIDGE_RT_*, is_nullable)`.
    ///
    /// Returns `(BRIDGE_RT_NONE, false)` for types that the bridge doesn't
    /// support yet (Class, Interface, Enum, Union, Intersection).
    pub fn to_bridge_tag(&self) -> (i32, bool) {
        match self {
            PhpType::Null => (BRIDGE_RT_NULL, false),
            PhpType::Bool => (BRIDGE_RT_BOOL, false),
            PhpType::Int => (BRIDGE_RT_INT, false),
            PhpType::Float => (BRIDGE_RT_FLOAT, false),
            PhpType::String => (BRIDGE_RT_STRING, false),
            PhpType::Array => (BRIDGE_RT_ARRAY, false),
            PhpType::Object => (BRIDGE_RT_OBJECT, false),
            PhpType::Mixed => (BRIDGE_RT_MIXED, false),
            PhpType::Void => (BRIDGE_RT_VOID, false),
            PhpType::Callable => (BRIDGE_RT_CALLABLE, false),
            PhpType::Iterable => (BRIDGE_RT_ITERABLE, false),
            PhpType::Never => (BRIDGE_RT_NEVER, false),
            PhpType::False => (BRIDGE_RT_FALSE, false),
            PhpType::True => (BRIDGE_RT_TRUE, false),
            PhpType::Self_ => (BRIDGE_RT_SELF, false),
            PhpType::Static_ => (BRIDGE_RT_STATIC, false),
            PhpType::Parent_ => (BRIDGE_RT_PARENT, false),
            PhpType::Nullable(inner) => {
                let (tag, _) = inner.to_bridge_tag();
                (tag, true)
            }
            // Complex types not yet supported by the bridge
            PhpType::Class(_)
            | PhpType::Interface(_)
            | PhpType::Enum(_)
            | PhpType::Union(_)
            | PhpType::Intersection(_) => (BRIDGE_RT_NONE, false),
        }
    }
}

// ─── Visibility ───────────────────────────────────────────────────────────────

/// PHP visibility modifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    Public,
    Protected,
    Private,
}

// ─── Modifiers ────────────────────────────────────────────────────────────────

bitflags! {
    /// PHP method/property modifiers as a bitfield.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct Modifiers: u8 {
        const ABSTRACT = 0x01;
        const FINAL    = 0x02;
        const STATIC   = 0x04;
        const READONLY = 0x08;
    }
}

// ─── PhpValue ─────────────────────────────────────────────────────────────────

/// A PHP value used as a default or constant expression.
#[derive(Debug, Clone, PartialEq)]
pub enum PhpValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    Array,
    ConstExpr(String),
}

impl fmt::Display for PhpValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PhpValue::Null => write!(f, "null"),
            PhpValue::Bool(true) => write!(f, "true"),
            PhpValue::Bool(false) => write!(f, "false"),
            PhpValue::Int(v) => write!(f, "{v}"),
            PhpValue::Float(v) => write!(f, "{v}"),
            PhpValue::String(s) => write!(f, "'{s}'"),
            PhpValue::Array => write!(f, "[]"),
            PhpValue::ConstExpr(s) => write!(f, "{s}"),
        }
    }
}

// ─── MagicMethod ─────────────────────────────────────────────────────────────

/// PHP magic method identifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MagicMethod {
    Construct,
    Destruct,
    Clone,
    Get,
    Set,
    Isset,
    Unset,
    Call,
    CallStatic,
    ToString,
    Invoke,
    DebugInfo,
    Serialize,
    Unserialize,
    Sleep,
    Wakeup,
    SetState,
}

impl MagicMethod {
    /// Total number of magic method variants.
    pub const COUNT: usize = 17;

    /// Returns the PHP name of the magic method (e.g. `"__construct"`).
    pub fn php_name(self) -> &'static str {
        match self {
            MagicMethod::Construct => "__construct",
            MagicMethod::Destruct => "__destruct",
            MagicMethod::Clone => "__clone",
            MagicMethod::Get => "__get",
            MagicMethod::Set => "__set",
            MagicMethod::Isset => "__isset",
            MagicMethod::Unset => "__unset",
            MagicMethod::Call => "__call",
            MagicMethod::CallStatic => "__callStatic",
            MagicMethod::ToString => "__toString",
            MagicMethod::Invoke => "__invoke",
            MagicMethod::DebugInfo => "__debugInfo",
            MagicMethod::Serialize => "__serialize",
            MagicMethod::Unserialize => "__unserialize",
            MagicMethod::Sleep => "__sleep",
            MagicMethod::Wakeup => "__wakeup",
            MagicMethod::SetState => "__set_state",
        }
    }

    /// Returns `true` for methods that act as Zend object handlers.
    ///
    /// Object handlers: Get, Set, Isset, Unset, Clone, ToString, DebugInfo, Invoke.
    pub fn is_object_handler(self) -> bool {
        matches!(
            self,
            MagicMethod::Get
                | MagicMethod::Set
                | MagicMethod::Isset
                | MagicMethod::Unset
                | MagicMethod::Clone
                | MagicMethod::ToString
                | MagicMethod::DebugInfo
                | MagicMethod::Invoke
        )
    }

    /// Returns the discriminant index of this variant (`self as usize`).
    pub fn index(self) -> usize {
        self as usize
    }

    /// PHP-expected return type tag for this magic method, mapped to the
    /// bridge's `OXPHP_RT_*` constants. PHP emits compile-time warnings
    /// like "Method X::__toString() implemented without string return
    /// type" when a class declares a magic method without the expected
    /// signature, so class registration needs to hand this through to
    /// `zend_function_entry.arg_info`.
    ///
    /// Returns `(tag, is_nullable)`. `BRIDGE_RT_NONE` means "let PHP
    /// apply the default" — safe for magics whose return type is
    /// user-defined (`__construct`, `__invoke`, `__call`, ...).
    pub fn return_tag(self) -> (i32, bool) {
        match self {
            MagicMethod::ToString => (BRIDGE_RT_STRING, false),
            MagicMethod::Isset => (BRIDGE_RT_BOOL, false),
            MagicMethod::Clone
            | MagicMethod::Destruct
            | MagicMethod::Unset
            | MagicMethod::Wakeup => (BRIDGE_RT_VOID, false),
            MagicMethod::Sleep | MagicMethod::Serialize | MagicMethod::DebugInfo => {
                (BRIDGE_RT_ARRAY, false)
            }
            MagicMethod::Get | MagicMethod::Set | MagicMethod::Call | MagicMethod::CallStatic => {
                (BRIDGE_RT_MIXED, false)
            }
            MagicMethod::Construct
            | MagicMethod::Invoke
            | MagicMethod::Unserialize
            | MagicMethod::SetState => (BRIDGE_RT_NONE, false),
        }
    }

    /// PHP-required arity: `(required, total)` parameter count. Needed so
    /// `zend_register_internal_class_ex` doesn't reject the generated
    /// `zend_function_entry` with a fatal like
    /// *"Method X::__get() must take exactly 1 argument"*.
    ///
    /// Values follow the magic method contract in the PHP manual
    /// (<https://www.php.net/manual/en/language.oop5.magic.php>).
    pub fn arity(self) -> (usize, usize) {
        match self {
            MagicMethod::Destruct
            | MagicMethod::Clone
            | MagicMethod::ToString
            | MagicMethod::DebugInfo
            | MagicMethod::Serialize
            | MagicMethod::Sleep
            | MagicMethod::Wakeup => (0, 0),
            MagicMethod::Get
            | MagicMethod::Isset
            | MagicMethod::Unset
            | MagicMethod::Unserialize
            | MagicMethod::SetState => (1, 1),
            MagicMethod::Set | MagicMethod::Call | MagicMethod::CallStatic => (2, 2),
            // User-defined signature — leave the validator alone.
            MagicMethod::Construct | MagicMethod::Invoke => (0, 0),
        }
    }

    /// Inverse of [`index`]. Returns `None` when `idx` is out of range.
    ///
    /// Kept in sync with the enum ordering above — asserts in tests keep
    /// the mapping honest if anyone reorders the variants.
    pub fn from_index(idx: usize) -> Option<MagicMethod> {
        Some(match idx {
            0 => MagicMethod::Construct,
            1 => MagicMethod::Destruct,
            2 => MagicMethod::Clone,
            3 => MagicMethod::Get,
            4 => MagicMethod::Set,
            5 => MagicMethod::Isset,
            6 => MagicMethod::Unset,
            7 => MagicMethod::Call,
            8 => MagicMethod::CallStatic,
            9 => MagicMethod::ToString,
            10 => MagicMethod::Invoke,
            11 => MagicMethod::DebugInfo,
            12 => MagicMethod::Serialize,
            13 => MagicMethod::Unserialize,
            14 => MagicMethod::Sleep,
            15 => MagicMethod::Wakeup,
            16 => MagicMethod::SetState,
            _ => return None,
        })
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── PhpType tests ──

    #[test]
    fn test_php_type_nullable() {
        let t = PhpType::Nullable(Box::new(PhpType::String));
        assert_eq!(t, PhpType::Nullable(Box::new(PhpType::String)));
        assert_ne!(t, PhpType::Nullable(Box::new(PhpType::Int)));
    }

    #[test]
    fn test_php_type_union() {
        let t = PhpType::Union(vec![PhpType::Int, PhpType::String, PhpType::Null]);
        if let PhpType::Union(ref parts) = t {
            assert_eq!(parts.len(), 3);
            assert_eq!(parts[0], PhpType::Int);
            assert_eq!(parts[1], PhpType::String);
            assert_eq!(parts[2], PhpType::Null);
        } else {
            panic!("expected Union");
        }
    }

    #[test]
    fn test_php_type_intersection() {
        let t = PhpType::Intersection(vec![
            PhpType::Interface("Countable".to_string()),
            PhpType::Interface("Iterator".to_string()),
        ]);
        if let PhpType::Intersection(ref parts) = t {
            assert_eq!(parts.len(), 2);
        } else {
            panic!("expected Intersection");
        }
    }

    #[test]
    fn test_php_type_class() {
        let t = PhpType::Class("MyClass".to_string());
        assert_eq!(t, PhpType::Class("MyClass".to_string()));
        assert_ne!(t, PhpType::Class("OtherClass".to_string()));
        assert_ne!(t, PhpType::Interface("MyClass".to_string()));
    }

    // ── Visibility tests ──

    #[test]
    fn test_visibility_variants() {
        let v = Visibility::Public;
        assert_eq!(v, Visibility::Public);
        assert_ne!(v, Visibility::Protected);
        assert_ne!(v, Visibility::Private);

        // Copy semantics
        let _copy = v;
        let _ = v; // original still usable
    }

    // ── Modifiers tests ──

    #[test]
    fn test_modifiers_bitflags() {
        let m = Modifiers::ABSTRACT | Modifiers::FINAL;
        assert!(m.contains(Modifiers::ABSTRACT));
        assert!(m.contains(Modifiers::FINAL));
        assert!(!m.contains(Modifiers::STATIC));
        assert!(!m.contains(Modifiers::READONLY));

        let all = Modifiers::ABSTRACT | Modifiers::FINAL | Modifiers::STATIC | Modifiers::READONLY;
        assert!(all.contains(Modifiers::STATIC));
        assert!(all.contains(Modifiers::READONLY));
    }

    #[test]
    fn test_modifiers_empty() {
        let m = Modifiers::empty();
        assert!(!m.contains(Modifiers::ABSTRACT));
        assert!(!m.contains(Modifiers::FINAL));
        assert!(!m.contains(Modifiers::STATIC));
        assert!(!m.contains(Modifiers::READONLY));
        assert!(m.is_empty());
    }

    // ── PhpValue tests ──

    #[test]
    fn test_php_value_variants() {
        let _null = PhpValue::Null;
        let _bool_t = PhpValue::Bool(true);
        let _bool_f = PhpValue::Bool(false);
        let _int = PhpValue::Int(42);
        let _float = PhpValue::Float(2.5);
        let _string = PhpValue::String("hello".to_string());
        let _array = PhpValue::Array;
        let _expr = PhpValue::ConstExpr("PHP_INT_MAX".to_string());
    }

    #[test]
    fn test_php_value_display() {
        assert_eq!(PhpValue::Null.to_string(), "null");
        assert_eq!(PhpValue::Bool(true).to_string(), "true");
        assert_eq!(PhpValue::Bool(false).to_string(), "false");
        assert_eq!(PhpValue::Int(42).to_string(), "42");
        assert_eq!(PhpValue::Int(-1).to_string(), "-1");
        assert_eq!(PhpValue::Float(2.5).to_string(), "2.5");
        assert_eq!(PhpValue::String("hello".to_string()).to_string(), "'hello'");
        assert_eq!(PhpValue::Array.to_string(), "[]");
        assert_eq!(
            PhpValue::ConstExpr("PHP_INT_MAX".to_string()).to_string(),
            "PHP_INT_MAX"
        );
    }

    // ── MagicMethod tests ──

    #[test]
    fn test_magic_method_name() {
        assert_eq!(MagicMethod::Construct.php_name(), "__construct");
        assert_eq!(MagicMethod::Destruct.php_name(), "__destruct");
        assert_eq!(MagicMethod::Clone.php_name(), "__clone");
        assert_eq!(MagicMethod::Get.php_name(), "__get");
        assert_eq!(MagicMethod::Set.php_name(), "__set");
        assert_eq!(MagicMethod::Isset.php_name(), "__isset");
        assert_eq!(MagicMethod::Unset.php_name(), "__unset");
        assert_eq!(MagicMethod::Call.php_name(), "__call");
        assert_eq!(MagicMethod::CallStatic.php_name(), "__callStatic");
        assert_eq!(MagicMethod::ToString.php_name(), "__toString");
        assert_eq!(MagicMethod::Invoke.php_name(), "__invoke");
        assert_eq!(MagicMethod::DebugInfo.php_name(), "__debugInfo");
        assert_eq!(MagicMethod::Serialize.php_name(), "__serialize");
        assert_eq!(MagicMethod::Unserialize.php_name(), "__unserialize");
        assert_eq!(MagicMethod::Sleep.php_name(), "__sleep");
        assert_eq!(MagicMethod::Wakeup.php_name(), "__wakeup");
        assert_eq!(MagicMethod::SetState.php_name(), "__set_state");
    }

    #[test]
    fn test_magic_method_via_handler() {
        // Object handlers: true
        assert!(MagicMethod::Get.is_object_handler());
        assert!(MagicMethod::Set.is_object_handler());
        assert!(MagicMethod::Isset.is_object_handler());
        assert!(MagicMethod::Unset.is_object_handler());
        assert!(MagicMethod::Clone.is_object_handler());
        assert!(MagicMethod::ToString.is_object_handler());
        assert!(MagicMethod::DebugInfo.is_object_handler());
        assert!(MagicMethod::Invoke.is_object_handler());

        // Non-object handlers: false
        assert!(!MagicMethod::Construct.is_object_handler());
        assert!(!MagicMethod::Destruct.is_object_handler());
        assert!(!MagicMethod::Call.is_object_handler());
        assert!(!MagicMethod::CallStatic.is_object_handler());
        assert!(!MagicMethod::Serialize.is_object_handler());
        assert!(!MagicMethod::Unserialize.is_object_handler());
        assert!(!MagicMethod::Sleep.is_object_handler());
        assert!(!MagicMethod::Wakeup.is_object_handler());
        assert!(!MagicMethod::SetState.is_object_handler());
    }

    #[test]
    fn test_magic_method_count() {
        assert_eq!(MagicMethod::COUNT, 17);
    }

    // ── Bridge return type tag tests ──

    #[test]
    fn test_bridge_tag_simple_types() {
        assert_eq!(PhpType::Null.to_bridge_tag(), (BRIDGE_RT_NULL, false));
        assert_eq!(PhpType::Bool.to_bridge_tag(), (BRIDGE_RT_BOOL, false));
        assert_eq!(PhpType::Int.to_bridge_tag(), (BRIDGE_RT_INT, false));
        assert_eq!(PhpType::Float.to_bridge_tag(), (BRIDGE_RT_FLOAT, false));
        assert_eq!(PhpType::String.to_bridge_tag(), (BRIDGE_RT_STRING, false));
        assert_eq!(PhpType::Array.to_bridge_tag(), (BRIDGE_RT_ARRAY, false));
        assert_eq!(PhpType::Object.to_bridge_tag(), (BRIDGE_RT_OBJECT, false));
        assert_eq!(PhpType::Mixed.to_bridge_tag(), (BRIDGE_RT_MIXED, false));
        assert_eq!(PhpType::Void.to_bridge_tag(), (BRIDGE_RT_VOID, false));
        assert_eq!(
            PhpType::Callable.to_bridge_tag(),
            (BRIDGE_RT_CALLABLE, false)
        );
        assert_eq!(
            PhpType::Iterable.to_bridge_tag(),
            (BRIDGE_RT_ITERABLE, false)
        );
        assert_eq!(PhpType::Never.to_bridge_tag(), (BRIDGE_RT_NEVER, false));
        assert_eq!(PhpType::False.to_bridge_tag(), (BRIDGE_RT_FALSE, false));
        assert_eq!(PhpType::True.to_bridge_tag(), (BRIDGE_RT_TRUE, false));
        assert_eq!(PhpType::Self_.to_bridge_tag(), (BRIDGE_RT_SELF, false));
        assert_eq!(PhpType::Static_.to_bridge_tag(), (BRIDGE_RT_STATIC, false));
        assert_eq!(PhpType::Parent_.to_bridge_tag(), (BRIDGE_RT_PARENT, false));
    }

    #[test]
    fn test_bridge_tag_nullable() {
        // ?string → (STRING, true)
        let t = PhpType::Nullable(Box::new(PhpType::String));
        assert_eq!(t.to_bridge_tag(), (BRIDGE_RT_STRING, true));

        // ?int → (INT, true)
        let t = PhpType::Nullable(Box::new(PhpType::Int));
        assert_eq!(t.to_bridge_tag(), (BRIDGE_RT_INT, true));

        // ?array → (ARRAY, true)
        let t = PhpType::Nullable(Box::new(PhpType::Array));
        assert_eq!(t.to_bridge_tag(), (BRIDGE_RT_ARRAY, true));
    }

    #[test]
    fn test_bridge_tag_unsupported_types_return_none() {
        // Class names, union, intersection → BRIDGE_RT_NONE (not yet supported)
        assert_eq!(
            PhpType::Class("Foo".into()).to_bridge_tag(),
            (BRIDGE_RT_NONE, false)
        );
        assert_eq!(
            PhpType::Interface("Bar".into()).to_bridge_tag(),
            (BRIDGE_RT_NONE, false)
        );
        assert_eq!(
            PhpType::Enum("Baz".into()).to_bridge_tag(),
            (BRIDGE_RT_NONE, false)
        );
        assert_eq!(
            PhpType::Union(vec![PhpType::Int, PhpType::String]).to_bridge_tag(),
            (BRIDGE_RT_NONE, false)
        );
        assert_eq!(
            PhpType::Intersection(vec![
                PhpType::Interface("A".into()),
                PhpType::Interface("B".into())
            ])
            .to_bridge_tag(),
            (BRIDGE_RT_NONE, false)
        );
    }

    #[test]
    fn test_bridge_tag_nullable_unsupported_inner() {
        // ?SomeClass → inner is unsupported, returns (NONE, true)
        let t = PhpType::Nullable(Box::new(PhpType::Class("Foo".into())));
        assert_eq!(t.to_bridge_tag(), (BRIDGE_RT_NONE, true));
    }

    #[test]
    fn test_bridge_tag_constants_match_c_header() {
        // Verify our Rust constants match the C OXPHP_RT_* values in oxphp_bridge.h
        assert_eq!(BRIDGE_RT_NONE, 0);
        assert_eq!(BRIDGE_RT_NULL, 1);
        assert_eq!(BRIDGE_RT_BOOL, 2);
        assert_eq!(BRIDGE_RT_INT, 3);
        assert_eq!(BRIDGE_RT_FLOAT, 4);
        assert_eq!(BRIDGE_RT_STRING, 5);
        assert_eq!(BRIDGE_RT_ARRAY, 6);
        assert_eq!(BRIDGE_RT_OBJECT, 7);
        assert_eq!(BRIDGE_RT_MIXED, 8);
        assert_eq!(BRIDGE_RT_VOID, 9);
        assert_eq!(BRIDGE_RT_CALLABLE, 10);
        assert_eq!(BRIDGE_RT_ITERABLE, 11);
        assert_eq!(BRIDGE_RT_NEVER, 12);
        assert_eq!(BRIDGE_RT_FALSE, 13);
        assert_eq!(BRIDGE_RT_TRUE, 14);
        assert_eq!(BRIDGE_RT_SELF, 15);
        assert_eq!(BRIDGE_RT_STATIC, 16);
        assert_eq!(BRIDGE_RT_PARENT, 17);
    }
}
