//! Memcached hook registrations.
//!
//! Hooks common Memcached extension methods for automatic cache span creation.

/// Register Memcached-related hooks. Returns the number of functions registered.
pub fn register() -> usize {
    super::register_hook("Memcached", "get");
    super::register_hook("Memcached", "set");
    super::register_hook("Memcached", "delete");
    super::register_hook("Memcached", "getMulti");
    super::register_hook("Memcached", "setMulti");
    5
}
