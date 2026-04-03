//! mysqli hook registrations.
//!
//! Hooks `mysqli::__construct`, `mysqli::query`, `mysqli::prepare`,
//! and `mysqli_stmt::execute` for automatic database span creation.

/// Register mysqli-related hooks. Returns the number of functions registered.
pub fn register() -> usize {
    super::register_hook("mysqli", "__construct");
    super::register_hook("mysqli", "query");
    super::register_hook("mysqli", "prepare");
    super::register_hook("mysqli_stmt", "execute");
    4
}
