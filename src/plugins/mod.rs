// Built-in plugins — each gated by Cargo features.
#[cfg(feature = "plugin-apm")]
pub mod apm;
#[cfg(feature = "plugin-async")]
pub mod async_plugin;
#[cfg(feature = "plugin-example")]
pub mod example;
#[cfg(feature = "plugin-otel")]
pub mod otel;
