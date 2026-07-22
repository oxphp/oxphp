//! mysqli hook registrations.
//!
//! Hooks `mysqli::__construct`, `mysqli::query`, `mysqli::prepare`,
//! `mysqli_stmt::prepare`, and `mysqli_stmt::execute` for automatic database
//! span creation. `mysqli_stmt::prepare` covers the `stmt_init()` idiom
//! (`$s = $m->stmt_init(); $s->prepare($sql); $s->execute();`) so that path's
//! SQL is captured on its own prepare span.

/// Register mysqli-related hooks. Returns the number of functions registered.
pub fn register() -> usize {
    super::register_hook("mysqli", "__construct");
    super::register_hook("mysqli", "query");
    super::register_hook("mysqli", "prepare");
    super::register_hook("mysqli_stmt", "prepare");
    super::register_hook("mysqli_stmt", "execute");
    5
}
