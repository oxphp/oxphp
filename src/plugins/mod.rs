// Built-in plugins — each gated by Cargo features.
#[cfg(feature = "plugin-apm")]
pub mod apm;
#[cfg(feature = "plugin-example")]
pub mod example;
#[cfg(feature = "plugin-otel")]
pub mod otel;
