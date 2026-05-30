//! Command-line interface for the `oxphp` binary.
//!
//! Parses `argv` via `lexopt` and maps it to a [`Command`]. Runtime behavior
//! (start server, print help, print version, validate config) is dispatched
//! from `main()` via [`dispatch`].
//!
//! Keep all CLI concerns in this module — argument parsing, help text,
//! version formatting, and the `config --check` validator live here so
//! `main.rs` stays focused on the server startup sequence.

use crate::config::Config;
use crate::types::BoxError;

/// Options for the `serve` role. Empty for now — a placeholder the
/// privilege-drop work extends with a `--user` target without reshaping
/// [`Command`].
#[derive(Debug, PartialEq, Eq, Default)]
pub struct ServeOptions {}

/// Parsed command from the process arguments.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Start the HTTP server. This is the default role when no command is
    /// given — bare `oxphp` is an implicit `oxphp serve`.
    Serve(ServeOptions),
    /// Print top-level help and exit 0.
    Help,
    /// Print version information and exit 0.
    Version,
    /// Print help for the `config` subcommand and exit 0.
    ConfigHelp,
    /// `config --check`: validate configuration, exit 0 on success or 1 on
    /// validation failure.
    ConfigCheck,
}

/// Parse `argv`, run any terminal command (help / version / config), and
/// return the parsed [`ServeOptions`] to `main` only when the server should
/// actually start.
///
/// This function **does not return** for non-`Serve` commands — it calls
/// `std::process::exit` with the appropriate code. That is safe here because
/// `dispatch` runs before any significant resources are allocated in `main`
/// (no logging guards, no Tokio runtime, no plugin state), so skipping
/// destructors has no observable effect.
///
/// Exit codes:
///   - `0` — help printed, version printed, or `config --check` passed
///   - `1` — `config --check` found problems
///   - `2` — unknown or conflicting CLI arguments
pub fn dispatch() -> ServeOptions {
    match parse() {
        Ok(Command::Serve(opts)) => opts,
        Ok(Command::Help) => {
            print_help();
            std::process::exit(0);
        }
        Ok(Command::Version) => {
            print_version();
            std::process::exit(0);
        }
        Ok(Command::ConfigHelp) => {
            print_config_help();
            std::process::exit(0);
        }
        Ok(Command::ConfigCheck) => match check_config() {
            Ok(()) => std::process::exit(0),
            Err(_) => std::process::exit(1),
        },
        Err(e) => {
            eprintln!("oxphp: {e}");
            eprintln!("try 'oxphp --help' for usage");
            std::process::exit(2);
        }
    }
}

/// Parse process arguments into a [`Command`].
///
/// Unknown flags and conflicting commands return an error describing the
/// problem. Normally callers should prefer [`dispatch`] — this is exposed
/// mainly for testing.
pub fn parse() -> Result<Command, BoxError> {
    parse_from(std::env::args_os().skip(1))
}

fn parse_from<I>(args: I) -> Result<Command, BoxError>
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    use lexopt::prelude::*;

    let mut parser = lexopt::Parser::from_args(args);
    let mut command: Option<Command> = None;

    while let Some(arg) = parser.next()? {
        let next = match arg {
            Short('h') | Long("help") => Command::Help,
            Short('v') | Long("version") => Command::Version,
            Value(v) if v == "serve" => {
                // The HTTP role. Like `config`, it cannot follow a top-level
                // flag — `oxphp --help serve` is nonsense.
                if command.is_some() {
                    return Err("unexpected subcommand 'serve' after top-level option".into());
                }
                return parse_serve_subcommand(&mut parser);
            }
            Value(v) if v == "config" => {
                // Hand off remaining args to the `config` subcommand parser.
                // `config` cannot be combined with top-level flags — if one
                // was seen already, surface it as a conflict.
                if command.is_some() {
                    return Err("unexpected subcommand 'config' after top-level option".into());
                }
                return parse_config_subcommand(&mut parser);
            }
            other => {
                return Err(format!("unexpected argument: {}", format_arg(&other)).into());
            }
        };

        // Conflict detection: `oxphp --help --version` is ambiguous.
        // First flag wins only when identical; otherwise error out.
        if let Some(existing) = &command {
            if existing != &next {
                return Err(format!(
                    "conflicting options: {existing:?} and {next:?} cannot be combined"
                )
                .into());
            }
        }
        command = Some(next);
    }

    Ok(command.unwrap_or(Command::Serve(ServeOptions::default())))
}

/// Parse the tail of arguments after `oxphp serve`. The parser is reused
/// mid-stream, mirroring [`parse_config_subcommand`].
fn parse_serve_subcommand(parser: &mut lexopt::Parser) -> Result<Command, BoxError> {
    // `serve` accepts no flags yet — the privilege-drop work adds `--user`
    // here. Until then any argument is unexpected.
    if let Some(arg) = parser.next()? {
        return Err(format!("unexpected argument to 'serve': {}", format_arg(&arg)).into());
    }
    Ok(Command::Serve(ServeOptions::default()))
}

/// Parse the tail of arguments after `oxphp config`. The parser is reused
/// mid-stream — lexopt happily continues from wherever the top-level loop
/// left off.
fn parse_config_subcommand(parser: &mut lexopt::Parser) -> Result<Command, BoxError> {
    use lexopt::prelude::*;

    let mut action: Option<Command> = None;
    while let Some(arg) = parser.next()? {
        let next = match arg {
            Short('h') | Long("help") => Command::ConfigHelp,
            Long("check") => Command::ConfigCheck,
            other => {
                return Err(
                    format!("unexpected argument to 'config': {}", format_arg(&other)).into(),
                );
            }
        };
        if let Some(existing) = &action {
            if existing != &next {
                return Err(format!(
                    "conflicting options under 'config': {existing:?} and {next:?}"
                )
                .into());
            }
        }
        action = Some(next);
    }
    // Bare `oxphp config` with no flags → print subcommand help.
    // Matches how `kubectl config` and similar tools behave.
    Ok(action.unwrap_or(Command::ConfigHelp))
}

fn format_arg(arg: &lexopt::Arg<'_>) -> String {
    use lexopt::prelude::*;
    match arg {
        Short(c) => format!("-{c}"),
        Long(name) => format!("--{name}"),
        Value(v) => v.to_string_lossy().into_owned(),
    }
}

/// Print top-level help text to stdout.
pub fn print_help() {
    // Hand-written to keep binary size tight. Small surface area doesn't
    // justify pulling in clap.
    println!(
        "OxPHP {version} — asynchronous PHP application server

USAGE:
    oxphp [OPTIONS]
    oxphp <COMMAND> [OPTIONS]

OPTIONS:
    -h, --help      Print this help and exit
    -v, --version   Print version information and exit

COMMANDS:
    serve           Start the HTTP server (default; same as bare 'oxphp')
    config          Configuration utilities (see 'oxphp config --help')

Without options or a command, OxPHP starts the HTTP server.",
        version = env!("CARGO_PKG_VERSION"),
    );
}

/// Print help for the `config` subcommand to stdout.
pub fn print_config_help() {
    println!(
        "oxphp config — configuration utilities

USAGE:
    oxphp config [OPTIONS]

OPTIONS:
    -h, --help   Print this help and exit
        --check  Validate configuration and report problems"
    );
}

/// Print version information to stdout.
pub fn print_version() {
    println!(
        "oxphp {version}
features: {features}",
        version = env!("CARGO_PKG_VERSION"),
        features = enabled_features(),
    );
}

fn enabled_features() -> String {
    // Order mirrors Cargo.toml for readability.
    let mut features: Vec<&'static str> = Vec::new();
    if cfg!(feature = "php") {
        features.push("php");
    }
    if cfg!(feature = "plugin-apm") {
        features.push("plugin-apm");
    }
    if cfg!(feature = "plugin-async") {
        features.push("plugin-async");
    }
    if cfg!(feature = "plugin-otel") {
        features.push("plugin-otel");
    }
    if features.is_empty() {
        "(none)".to_string()
    } else {
        features.join(", ")
    }
}

/// Validate configuration and report the result. Returns `Ok(())` when the
/// config parses and all filesystem checks pass, `Err` otherwise — the caller
/// decides the exit code.
///
/// Only filesystem sanity checks are performed: path existence and
/// file/directory kind. PHP runtime, TLS handshake, and network binding are
/// intentionally out of scope.
pub fn check_config() -> Result<(), BoxError> {
    let config = match Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            println!("config: INVALID");
            println!("  - {e}");
            return Err("config parse error".into());
        }
    };

    let errors = config.validate();
    if errors.is_empty() {
        println!("config: OK");
        Ok(())
    } else {
        println!("config: INVALID");
        for e in &errors {
            println!("  - {e}");
        }
        Err(format!("{} validation error(s)", errors.len()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn no_args_serves() {
        // Bare `oxphp` stays an implicit `serve` — hard backward-compat
        // constraint; the published image runs `CMD ["oxphp"]`.
        assert_eq!(
            parse_from(args(&[])).unwrap(),
            Command::Serve(ServeOptions::default())
        );
    }

    #[test]
    fn serve_subcommand() {
        assert_eq!(
            parse_from(args(&["serve"])).unwrap(),
            Command::Serve(ServeOptions::default())
        );
    }

    #[test]
    fn serve_rejects_unknown_flag() {
        // `serve` takes zero flags for now; any extra arg is an error.
        let err = parse_from(args(&["serve", "--bogus"])).unwrap_err();
        assert!(err.to_string().contains("'serve'"));
    }

    #[test]
    fn serve_cannot_follow_top_level_flag() {
        // `oxphp --help serve` is nonsense — reject, mirroring `config`.
        let err = parse_from(args(&["--help", "serve"])).unwrap_err();
        assert!(err.to_string().contains("serve"));
    }

    #[test]
    fn short_help() {
        assert_eq!(parse_from(args(&["-h"])).unwrap(), Command::Help);
    }

    #[test]
    fn long_help() {
        assert_eq!(parse_from(args(&["--help"])).unwrap(), Command::Help);
    }

    #[test]
    fn short_version() {
        assert_eq!(parse_from(args(&["-v"])).unwrap(), Command::Version);
    }

    #[test]
    fn long_version() {
        assert_eq!(parse_from(args(&["--version"])).unwrap(), Command::Version);
    }

    #[test]
    fn config_check_subcommand() {
        assert_eq!(
            parse_from(args(&["config", "--check"])).unwrap(),
            Command::ConfigCheck
        );
    }

    #[test]
    fn bare_config_prints_subcommand_help() {
        assert_eq!(parse_from(args(&["config"])).unwrap(), Command::ConfigHelp);
    }

    #[test]
    fn config_help_subcommand() {
        assert_eq!(
            parse_from(args(&["config", "--help"])).unwrap(),
            Command::ConfigHelp
        );
        assert_eq!(
            parse_from(args(&["config", "-h"])).unwrap(),
            Command::ConfigHelp
        );
    }

    #[test]
    fn config_unknown_subflag_errors() {
        let err = parse_from(args(&["config", "--bogus"])).unwrap_err();
        assert!(err.to_string().contains("'config'"));
    }

    #[test]
    fn config_cannot_follow_top_level_flag() {
        // `oxphp --help config` is nonsense — reject rather than silently
        // ignore the subcommand.
        let err = parse_from(args(&["--help", "config"])).unwrap_err();
        assert!(err.to_string().contains("config"));
    }

    #[test]
    fn old_check_config_flag_no_longer_recognized() {
        // Sanity: the pre-subcommand flag must be gone.
        let err = parse_from(args(&["--check-config"])).unwrap_err();
        assert!(err.to_string().contains("check-config"));
    }

    #[test]
    fn unknown_flag_errors() {
        let err = parse_from(args(&["--nope"])).unwrap_err();
        assert!(err.to_string().contains("--nope") || err.to_string().contains("nope"));
    }

    #[test]
    fn positional_arg_errors() {
        let err = parse_from(args(&["start"])).unwrap_err();
        assert!(err.to_string().to_lowercase().contains("unexpected"));
    }

    #[test]
    fn conflicting_flags_error() {
        let err = parse_from(args(&["--help", "--version"])).unwrap_err();
        assert!(err.to_string().contains("conflict"));
    }

    #[test]
    fn repeated_same_flag_is_ok() {
        assert_eq!(parse_from(args(&["-h", "--help"])).unwrap(), Command::Help);
    }
}
