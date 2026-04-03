//! cURL hook registrations.
//!
//! Hooks `curl_init`, `curl_setopt`, `curl_exec`, and `curl_multi_exec`
//! for automatic HTTP client span creation.

/// Register cURL-related hooks. Returns the number of functions registered.
pub fn register() -> usize {
    super::register_hook("", "curl_init");
    super::register_hook("", "curl_setopt");
    super::register_hook("", "curl_exec");
    super::register_hook("", "curl_multi_exec");
    4
}
