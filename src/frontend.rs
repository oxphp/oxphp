//! Execution frontends.
//!
//! HTTP is the default role (`serve`); this module hosts the non-HTTP roles.
//! The first such role is the one-shot CLI: [`run_cli`] executes a single PHP
//! script under CLI semantics and exits with the script's code. The
//! implementation is self-contained — it builds its own lightweight engine —
//! so it never touches the HTTP startup path and can later fold into a
//! `Frontend` trait without re-gluing.

use crate::cli::RunOptions;

#[cfg(feature = "php")]
pub mod cli_oneshot;

/// Execute the `run` role and return the process exit code.
#[cfg(feature = "php")]
pub fn run_cli(opts: RunOptions) -> i32 {
    cli_oneshot::run(opts)
}

/// Without the `php` feature there is no engine to execute a script under.
#[cfg(not(feature = "php"))]
pub fn run_cli(_opts: RunOptions) -> i32 {
    eprintln!("oxphp: this build has no PHP support; 'run' is unavailable");
    1
}
