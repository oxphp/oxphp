use std::sync::Arc;

/// Result of a decorator's on_begin() call.
#[derive(Debug, Clone, PartialEq)]
pub enum DecoratorAction {
    Continue,
    Reject(String),
}

/// Which PHP attribute targets this decorator supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttributeTargets(u32);

impl AttributeTargets {
    pub const FUNCTION: Self = Self(0x01);
    pub const METHOD: Self = Self(0x02);
    pub const CLASS: Self = Self(0x04);
    pub const ALL: Self = Self(0x01 | 0x02 | 0x04);

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(bits & 0x07)
    }
}

impl std::ops::BitOr for AttributeTargets {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

/// Context passed to decorator on_begin/on_end.
/// String fields use Arc<str> — allocated once during resolution, reused across calls.
#[derive(Debug, Clone)]
pub struct DecoratorCallContext {
    pub target: Arc<str>,
    pub class: Option<Arc<str>>,
    pub method: Option<Arc<str>>,
    pub function: Option<Arc<str>>,
    pub object_id: u64,
    pub request_id: String,
    pub trace_id: String,
    pub timestamp_ns: u64,
}

/// Result of the decorated function execution, passed to on_end.
#[derive(Debug, Clone)]
pub struct DecoratorCallResult {
    pub success: bool,
    pub elapsed_ns: u64,
    pub exception_class: Option<String>,
}

/// One decoded PHP attribute constructor argument.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrArg {
    Int(i64),
    Float(f64),
    Str(Arc<str>),
    Bool(bool),
    Null,
}

/// Decoded constructor arguments of one attribute occurrence, in
/// source-declaration order. Read once at resolve time and handed to
/// [`Decorator::configure`]. Owned (no FFI / no lifetime) so decorators
/// and tests can build and inspect it without the C bridge.
///
/// Each entry pairs the argument's value with its parameter name. The
/// name is `Some` only for arguments written `name:`-style
/// (`#[Attr(ms: 250)]`); positional arguments carry `None`. Zend stores
/// attribute arguments in the order they appear in source, *not* in
/// constructor-parameter order — so for any attribute with more than one
/// argument, read named arguments by name ([`int_named`](Self::int_named)
/// et al.) and positional arguments by index ([`int`](Self::int) et al.).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AttrArgs {
    args: Vec<(Option<Arc<str>>, AttrArg)>,
}

impl AttrArgs {
    /// Build from positional values only — every argument's name is
    /// `None`. Used by tests and the host build (where attribute args
    /// are not read from Zend).
    pub fn positional(args: Vec<AttrArg>) -> Self {
        Self {
            args: args.into_iter().map(|v| (None, v)).collect(),
        }
    }

    /// Build from `(name, value)` pairs in source-declaration order.
    /// `name` is `Some` for `name:`-style arguments, `None` for
    /// positional ones. Used by the resolve layer.
    pub fn from_pairs(args: Vec<(Option<Arc<str>>, AttrArg)>) -> Self {
        Self { args }
    }

    /// Number of arguments.
    pub fn len(&self) -> usize {
        self.args.len()
    }

    /// True when no arguments were supplied (a bare `#[Attr]`).
    pub fn is_empty(&self) -> bool {
        self.args.is_empty()
    }

    /// Integer at `idx`. Only an `Int` matches — no coercion.
    pub fn int(&self, idx: usize) -> Option<i64> {
        match self.args.get(idx) {
            Some((_, AttrArg::Int(v))) => Some(*v),
            _ => None,
        }
    }

    /// Float at `idx`. Accepts `Float` and integer-valued `Int`.
    pub fn float(&self, idx: usize) -> Option<f64> {
        match self.args.get(idx) {
            Some((_, AttrArg::Float(v))) => Some(*v),
            Some((_, AttrArg::Int(v))) => Some(*v as f64),
            _ => None,
        }
    }

    /// String slice at `idx`.
    pub fn str(&self, idx: usize) -> Option<&str> {
        match self.args.get(idx) {
            Some((_, AttrArg::Str(s))) => Some(s),
            _ => None,
        }
    }

    /// Boolean at `idx`. Only a `Bool` matches — no coercion.
    pub fn bool(&self, idx: usize) -> Option<bool> {
        match self.args.get(idx) {
            Some((_, AttrArg::Bool(v))) => Some(*v),
            _ => None,
        }
    }

    /// Value of the argument named `name`, or `None` if absent. Matches
    /// only `name:`-style arguments.
    fn named(&self, name: &str) -> Option<&AttrArg> {
        self.args.iter().find_map(|(n, v)| match n {
            Some(n) if n.as_ref() == name => Some(v),
            _ => None,
        })
    }

    /// Integer of the argument named `name`. Only an `Int` matches.
    pub fn int_named(&self, name: &str) -> Option<i64> {
        match self.named(name) {
            Some(AttrArg::Int(v)) => Some(*v),
            _ => None,
        }
    }

    /// Float of the argument named `name`. Accepts `Float` and
    /// integer-valued `Int`.
    pub fn float_named(&self, name: &str) -> Option<f64> {
        match self.named(name) {
            Some(AttrArg::Float(v)) => Some(*v),
            Some(AttrArg::Int(v)) => Some(*v as f64),
            _ => None,
        }
    }

    /// String slice of the argument named `name`.
    pub fn str_named(&self, name: &str) -> Option<&str> {
        match self.named(name) {
            Some(AttrArg::Str(s)) => Some(s),
            _ => None,
        }
    }

    /// Boolean of the argument named `name`. Only a `Bool` matches.
    pub fn bool_named(&self, name: &str) -> Option<bool> {
        match self.named(name) {
            Some(AttrArg::Bool(v)) => Some(*v),
            _ => None,
        }
    }
}

/// Trait for Rust-native decorators, registered by plugins.
pub trait Decorator: Send + Sync {
    /// Fully qualified PHP attribute class name (e.g. "App\\Profiler\\Timer").
    fn attribute_name(&self) -> &str;

    /// Which attribute targets this decorator supports (for registry optimization).
    fn targets(&self) -> AttributeTargets;

    /// Called before the decorated function executes.
    fn on_begin(&self, ctx: &DecoratorCallContext) -> DecoratorAction;

    /// Called after the decorated function executes.
    fn on_end(&self, ctx: &DecoratorCallContext, result: &DecoratorCallResult);

    /// Build a configured instance for one attribute occurrence.
    ///
    /// Called once per `(function, attribute)` at resolve time with the
    /// attribute's decoded constructor arguments. Return a new instance
    /// carrying them — it then receives `on_begin`/`on_end` for every
    /// call of the decorated function. The default returns `None`: no
    /// per-attribute configuration, the registered instance is shared
    /// as-is.
    ///
    /// `args` preserves source-declaration order, which is *not* the
    /// constructor-parameter order when callers mix positions and
    /// `name:`-style arguments. Read named arguments by name
    /// ([`AttrArgs::int_named`] et al.) and positional ones by index
    /// ([`AttrArgs::int`] et al.); for a single-argument attribute the
    /// two are equivalent.
    fn configure(&self, args: &AttrArgs) -> Option<Arc<dyn Decorator>> {
        let _ = args;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decorator_action_variants() {
        let action = DecoratorAction::Continue;
        assert_eq!(action, DecoratorAction::Continue);

        let action = DecoratorAction::Reject("circuit open".into());
        assert_eq!(action, DecoratorAction::Reject("circuit open".into()));
    }

    #[test]
    fn test_attribute_targets_bitflags() {
        let targets = AttributeTargets::FUNCTION | AttributeTargets::METHOD;
        assert!(targets.contains(AttributeTargets::FUNCTION));
        assert!(targets.contains(AttributeTargets::METHOD));
        assert!(!targets.contains(AttributeTargets::CLASS));

        let all = AttributeTargets::ALL;
        assert!(all.contains(AttributeTargets::FUNCTION));
        assert!(all.contains(AttributeTargets::METHOD));
        assert!(all.contains(AttributeTargets::CLASS));
    }

    #[test]
    fn test_call_context_arc_str_reuse() {
        let target: Arc<str> = Arc::from("App\\Service::method");
        let ctx1 = DecoratorCallContext {
            target: Arc::clone(&target),
            class: Some(Arc::from("App\\Service")),
            method: Some(Arc::from("method")),
            function: None,
            object_id: 42,
            request_id: "abc123".into(),
            trace_id: "trace-1".into(),
            timestamp_ns: 1000,
        };
        let ctx2 = DecoratorCallContext {
            target: Arc::clone(&target),
            class: ctx1.class.clone(),
            method: ctx1.method.clone(),
            function: None,
            object_id: 43,
            request_id: "def456".into(),
            trace_id: "trace-2".into(),
            timestamp_ns: 2000,
        };
        assert!(Arc::ptr_eq(&ctx1.target, &ctx2.target));
    }

    #[test]
    fn test_call_result_fields() {
        let result = DecoratorCallResult {
            success: true,
            elapsed_ns: 5_000_000,
            exception_class: None,
        };
        assert!(result.success);
        assert_eq!(result.elapsed_ns, 5_000_000);
        assert!(result.exception_class.is_none());

        let result = DecoratorCallResult {
            success: false,
            elapsed_ns: 1_000,
            exception_class: Some("RuntimeException".into()),
        };
        assert!(!result.success);
        assert_eq!(result.exception_class.as_deref(), Some("RuntimeException"));
    }

    #[test]
    fn test_attr_args_accessors() {
        let args = AttrArgs::positional(vec![
            AttrArg::Int(250),
            AttrArg::Str(Arc::from("checkout")),
            AttrArg::Float(0.5),
            AttrArg::Bool(true),
            AttrArg::Null,
        ]);
        assert_eq!(args.len(), 5);
        assert!(!args.is_empty());
        assert_eq!(args.int(0), Some(250));
        assert_eq!(args.str(1), Some("checkout"));
        assert_eq!(args.float(2), Some(0.5));
        assert_eq!(args.bool(3), Some(true));
        // Int coerces to float; the reverse does not coerce.
        assert_eq!(args.float(0), Some(250.0));
        assert_eq!(args.int(2), None);
        assert_eq!(args.bool(0), None);
        // Null and out-of-range read as None through every accessor.
        assert_eq!(args.int(4), None);
        assert_eq!(args.str(4), None);
        assert_eq!(args.bool(4), None);
        assert_eq!(args.int(9), None);
        assert_eq!(args.str(0), None);
    }

    #[test]
    fn test_attr_args_empty() {
        let args = AttrArgs::default();
        assert!(args.is_empty());
        assert_eq!(args.int(0), None);
        assert_eq!(args.str(0), None);
        assert_eq!(args.int_named("ms"), None);
        assert_eq!(args.str_named("label"), None);
    }

    #[test]
    fn test_attr_args_named_accessors() {
        // `#[Foo(ms: 250, label: "checkout", rate: 0.5, on: true)]`
        let args = AttrArgs::from_pairs(vec![
            (Some(Arc::from("ms")), AttrArg::Int(250)),
            (
                Some(Arc::from("label")),
                AttrArg::Str(Arc::from("checkout")),
            ),
            (Some(Arc::from("rate")), AttrArg::Float(0.5)),
            (Some(Arc::from("on")), AttrArg::Bool(true)),
        ]);
        assert_eq!(args.int_named("ms"), Some(250));
        assert_eq!(args.str_named("label"), Some("checkout"));
        assert_eq!(args.float_named("rate"), Some(0.5));
        assert_eq!(args.bool_named("on"), Some(true));
        // Int coerces to float through the named accessor too.
        assert_eq!(args.float_named("ms"), Some(250.0));
        // Type mismatch and unknown name both read as None.
        assert_eq!(args.str_named("ms"), None);
        assert_eq!(args.int_named("missing"), None);
        // Positional accessors still index by source order.
        assert_eq!(args.int(0), Some(250));
        assert_eq!(args.str(1), Some("checkout"));
    }

    #[test]
    fn test_attr_args_reordered_named_map_by_name_not_index() {
        // `#[Foo(b: 1, a: 2)]` on `__construct($a, $b)`: Zend keeps
        // source order, so positional reads see them swapped, but
        // name lookup recovers the intended mapping.
        let args = AttrArgs::from_pairs(vec![
            (Some(Arc::from("b")), AttrArg::Int(1)),
            (Some(Arc::from("a")), AttrArg::Int(2)),
        ]);
        // Positional: index 0 is `b`, index 1 is `a` (the bug this fixes).
        assert_eq!(args.int(0), Some(1));
        assert_eq!(args.int(1), Some(2));
        // Named: `a` and `b` resolve correctly regardless of order.
        assert_eq!(args.int_named("a"), Some(2));
        assert_eq!(args.int_named("b"), Some(1));
    }

    #[test]
    fn test_attr_args_positional_have_no_names() {
        // `positional()` records no names — named lookup misses,
        // index lookup hits.
        let args = AttrArgs::positional(vec![AttrArg::Int(7)]);
        assert_eq!(args.int(0), Some(7));
        assert_eq!(args.int_named("anything"), None);
    }

    #[test]
    fn test_default_configure_returns_none() {
        struct Bare;
        impl Decorator for Bare {
            fn attribute_name(&self) -> &str {
                "App\\Bare"
            }
            fn targets(&self) -> AttributeTargets {
                AttributeTargets::ALL
            }
            fn on_begin(&self, _: &DecoratorCallContext) -> DecoratorAction {
                DecoratorAction::Continue
            }
            fn on_end(&self, _: &DecoratorCallContext, _: &DecoratorCallResult) {}
        }
        // Default impl ignores args and opts out of per-attribute config.
        assert!(Bare
            .configure(&AttrArgs::positional(vec![AttrArg::Int(1)]))
            .is_none());
    }
}
