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
}
