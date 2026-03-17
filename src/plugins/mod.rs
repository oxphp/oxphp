// Built-in plugins — each gated by Cargo features.
#[cfg(feature = "plugin-example")]
pub mod example;
#[cfg(feature = "plugin-otel")]
pub mod otel;
