//! PDO hook registrations.
//!
//! Hooks `PDO::__construct`, `PDO::query`, `PDO::exec`, `PDO::prepare`,
//! and `PDOStatement::execute` for automatic database span creation.

/// Register PDO-related hooks. Returns the number of functions registered.
pub fn register() -> usize {
    super::register_hook("PDO", "__construct");
    super::register_hook("PDO", "query");
    super::register_hook("PDO", "exec");
    super::register_hook("PDO", "prepare");
    super::register_hook("PDOStatement", "execute");
    5
}
