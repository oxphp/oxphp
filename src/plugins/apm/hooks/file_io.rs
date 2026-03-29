//! File I/O hook registrations.
//!
//! Hooks common file functions for automatic I/O span creation.

/// Register file I/O hooks. Returns the number of functions registered.
pub fn register() -> usize {
    super::register_hook("", "fopen");
    super::register_hook("", "fread");
    super::register_hook("", "fwrite");
    super::register_hook("", "file_get_contents");
    super::register_hook("", "file_put_contents");
    5
}
