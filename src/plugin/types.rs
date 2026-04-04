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
        let _float = PhpValue::Float(3.14);
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
        assert_eq!(PhpValue::Float(3.14).to_string(), "3.14");
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
}
