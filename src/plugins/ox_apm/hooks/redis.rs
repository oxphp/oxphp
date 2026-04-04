//! Redis hook registrations.
//!
//! Hooks common Redis extension methods for automatic cache span creation.

/// Register Redis-related hooks. Returns the number of functions registered.
pub fn register() -> usize {
    super::register_hook("Redis", "connect");
    super::register_hook("Redis", "get");
    super::register_hook("Redis", "set");
    super::register_hook("Redis", "del");
    super::register_hook("Redis", "mget");
    super::register_hook("Redis", "mset");
    super::register_hook("Redis", "hget");
    super::register_hook("Redis", "hset");
    super::register_hook("Redis", "lpush");
    super::register_hook("Redis", "rpush");
    10
}
