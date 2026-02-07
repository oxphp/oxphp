/// Convenience alias for a boxed error that is Send + Sync.
/// Used throughout the codebase for fallible operations.
pub type BoxError = Box<dyn std::error::Error + Send + Sync>;
