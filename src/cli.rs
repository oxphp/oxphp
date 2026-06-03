//! Command-line interface for the `oxphp` binary.
//!
//! Parses `argv` via `lexopt` and maps it to a [`Command`]. Runtime behavior
//! (start server, print help, print version, validate config) is dispatched
//! from `main()` via [`dispatch`].
//!
//! Keep all CLI concerns in this module — argument parsing, help text,
//! version formatting, and the `config --check` validator live here so
//! `main.rs` stays focused on the server startup sequence.

use std::ffi::OsString;
use std::path::PathBuf;

use crate::config::Config;
use crate::types::BoxError;

/// Options for the `serve` role.
#[derive(Debug, PartialEq, Eq, Default)]
pub struct ServeOptions {
    /// Drop OS privileges to this user/group after binding the listeners.
    /// `Some` only when `--user` was given. The bind happens as the starting
    /// user (root, for privileged ports); the drop is irreversible.
    pub drop_to: Option<DropTarget>,
}

/// Resolved target of `serve --user=<name|name:group|uid|uid:gid>`.
///
/// Parsing resolves names to numeric ids eagerly (at CLI-parse time) so an
/// unknown user/group fails fast, before any startup work. The user name is
/// kept for `initgroups()`; it is `None` only for a bare numeric uid with no
/// matching passwd entry, in which case the privilege drop clears
/// supplementary groups instead of expanding them.
#[derive(Debug, PartialEq, Eq)]
pub struct DropTarget {
    pub uid: u32,
    pub gid: u32,
    pub user: Option<std::ffi::CString>,
}

#[cfg(unix)]
impl std::str::FromStr for DropTarget {
    type Err = BoxError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (user_part, group_part) = match s.split_once(':') {
            Some((u, g)) => (u, Some(g)),
            None => (s, None),
        };
        if user_part.is_empty() {
            return Err(format!("--user: empty user in '{s}'").into());
        }

        // Resolve the user → (uid, its primary gid, its name for initgroups).
        // A numeric uid is taken verbatim (Docker `--user` convention) and
        // reverse-resolved for its name/default gid; a name must resolve.
        let (uid, primary_gid, name) = if let Ok(uid) = user_part.parse::<u32>() {
            match lookup_passwd_by_uid(uid) {
                Some(pw) => (uid, Some(pw.gid), Some(pw.name)),
                None => (uid, None, None),
            }
        } else {
            let pw = lookup_passwd_by_name(user_part)
                .ok_or_else(|| format!("--user: unknown user '{user_part}'"))?;
            (pw.uid, Some(pw.gid), Some(pw.name))
        };

        // Resolve the group: explicit (numeric or name) or the user's primary.
        let gid = match group_part {
            Some("") => {
                return Err(format!("--user: empty group in '{s}'").into());
            }
            Some(g) => match g.parse::<u32>() {
                Ok(gid) => gid,
                Err(_) => {
                    lookup_group_by_name(g).ok_or_else(|| format!("--user: unknown group '{g}'"))?
                }
            },
            None => primary_gid.ok_or_else(|| {
                format!(
                    "--user: cannot determine group for uid {uid} (no passwd entry); use uid:gid"
                )
            })?,
        };

        Ok(DropTarget {
            uid,
            gid,
            user: name,
        })
    }
}

#[cfg(not(unix))]
impl std::str::FromStr for DropTarget {
    type Err = BoxError;

    fn from_str(_s: &str) -> Result<Self, Self::Err> {
        Err("--user is not supported on this platform".into())
    }
}

#[cfg(unix)]
struct PasswdEntry {
    uid: u32,
    gid: u32,
    name: std::ffi::CString,
}

/// Look up a passwd entry by name via `getpwnam_r`. Mirrors the buffer pattern
/// in `startup_identity` (fixed 1024-byte buffer, no ERANGE retry).
#[cfg(unix)]
fn lookup_passwd_by_name(name: &str) -> Option<PasswdEntry> {
    let c_name = std::ffi::CString::new(name).ok()?;
    let mut buf = vec![0u8; 1024];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: getpwnam_r writes into `pwd`/`buf` (owned here); `result` is set
    // to NULL on miss / non-zero return.
    let rc = unsafe {
        libc::getpwnam_r(
            c_name.as_ptr(),
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    // SAFETY: pw_name points into `buf` for the lifetime of `pwd`.
    let name = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) }.to_owned();
    Some(PasswdEntry {
        uid: pwd.pw_uid,
        gid: pwd.pw_gid,
        name,
    })
}

/// Look up a passwd entry by uid via `getpwuid_r` (reverse resolution for a
/// numeric `--user`, to recover the name for `initgroups` and a default gid).
#[cfg(unix)]
fn lookup_passwd_by_uid(uid: u32) -> Option<PasswdEntry> {
    let mut buf = vec![0u8; 1024];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    // SAFETY: same contract as lookup_passwd_by_name.
    let rc = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    // SAFETY: pw_name points into `buf` for the lifetime of `pwd`.
    let name = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) }.to_owned();
    Some(PasswdEntry {
        uid: pwd.pw_uid,
        gid: pwd.pw_gid,
        name,
    })
}

/// Look up a group's gid by name via `getgrnam_r`.
#[cfg(unix)]
fn lookup_group_by_name(name: &str) -> Option<u32> {
    let c_name = std::ffi::CString::new(name).ok()?;
    let mut buf = vec![0u8; 1024];
    let mut grp: libc::group = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::group = std::ptr::null_mut();
    // SAFETY: getgrnam_r writes into `grp`/`buf` (owned here); `result` is set
    // to NULL on miss / non-zero return.
    let rc = unsafe {
        libc::getgrnam_r(
            c_name.as_ptr(),
            &mut grp,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };
    if rc != 0 || result.is_null() {
        return None;
    }
    Some(grp.gr_gid)
}

/// Options for the `run` role — execute one PHP file to completion under CLI
/// semantics, then exit with the script's exit code.
#[derive(Debug, PartialEq, Eq)]
pub struct RunOptions {
    /// Path to the PHP script to execute.
    pub script: PathBuf,
    /// The script's argument tail (everything after the script path on the
    /// command line). Passed through verbatim to PHP as `$argv[1..]`; flags
    /// like `--verbose` reach the script, not the oxphp argument parser.
    pub args: Vec<OsString>,
    /// `-d key=value` ini overrides, applied before the script runs (php-CLI
    /// parity). These are oxphp's own flags, consumed before the script path.
    pub ini: Vec<(String, String)>,
    /// Drop OS privileges to this user/group before MINIT and script execution
    /// when `--user` was given (k8s `Job` as non-root). Mirrors
    /// `ServeOptions::drop_to`; the drop itself is performed by `main` before
    /// calling `run_cli`, because `privdrop` lives in the binary crate.
    pub user: Option<DropTarget>,
}

/// The runtime role selected by [`dispatch`]. Exactly one role runs per
/// process — there is no HTTP listener in the `Run` role, and no one-shot
/// execution in the `Serve` role.
#[derive(Debug, PartialEq, Eq)]
pub enum Role {
    /// Start the HTTP server.
    Serve(ServeOptions),
    /// Execute a single PHP script under CLI semantics and exit.
    Run(RunOptions),
}

/// Parsed command from the process arguments.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Start the HTTP server. This is the default role when no command is
    /// given — bare `oxphp` is an implicit `oxphp serve`.
    Serve(ServeOptions),
    /// Execute a single PHP script under CLI semantics and exit with the
    /// script's exit code.
    Run(RunOptions),
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
/// return the selected [`Role`] to `main` only when the process should
/// actually run (start the server or execute a script).
///
/// This function **does not return** for terminal commands (help / version /
/// `config --check` / bad args) — it calls `std::process::exit` with the
/// appropriate code. That is safe here because `dispatch` runs before any
/// significant resources are allocated in `main` (no logging guards, no Tokio
/// runtime, no plugin state), so skipping destructors has no observable effect.
///
/// Exit codes:
///   - `0` — help printed, version printed, or `config --check` passed
///   - `1` — `config --check` found problems
///   - `2` — unknown, malformed, or out-of-role CLI arguments
pub fn dispatch() -> Role {
    match parse() {
        Ok(Command::Serve(opts)) => Role::Serve(opts),
        Ok(Command::Run(opts)) => Role::Run(opts),
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
/// Unknown flags and out-of-role options return an error describing the
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

    // Cross-cutting flags collected before the role is known. They are applied
    // to whichever role the first positional resolves to, then validated
    // against that role's allow-set.
    let mut ini: Vec<(String, String)> = Vec::new();
    let mut user: Option<DropTarget> = None;

    while let Some(arg) = parser.next()? {
        match arg {
            // Terminal flags short-circuit on first occurrence (first wins).
            Short('h') | Long("help") => return Ok(Command::Help),
            Short('v') | Long("version") => return Ok(Command::Version),
            // Cross-cutting collected flags.
            Short('d') => ini.push(parse_ini_define(parser.value()?)),
            Long("user") => user = Some(parse_user(&mut parser)?),
            // Reserved keywords (exact match) select a subcommand role.
            Value(v) if v == "serve" => return finish_serve(&mut parser, ini, user),
            Value(v) if v == "run" => return finish_run(&mut parser, ini, user),
            Value(v) if v == "config" => {
                reject_script_flags("config", &ini, &user)?;
                return parse_config_subcommand(&mut parser);
            }
            // Any other first positional is a script path → implicit run.
            Value(v) => return build_run(v, &mut parser, ini, user),
            other => return Err(format!("unexpected argument: {}", format_arg(&other)).into()),
        }
    }

    // No positional → serve. `-d` has no meaning without a script to run.
    if !ini.is_empty() {
        return Err(d_requires_script("serve"));
    }
    Ok(Command::Serve(ServeOptions { drop_to: user }))
}

/// Parse one `-d key[=value]` token. A bare key (no `=`) maps to "1" (php-cli).
fn parse_ini_define(raw: OsString) -> (String, String) {
    let text = raw.to_string_lossy();
    match text.split_once('=') {
        Some((k, v)) => (k.to_string(), v.to_string()),
        None => (text.into_owned(), "1".to_string()),
    }
}

/// Parse a `--user=<spec>` value into a resolved `DropTarget`. The per-flag
/// parsing lives in one place (mirrors `parse_ini_define`).
fn parse_user(parser: &mut lexopt::Parser) -> Result<DropTarget, BoxError> {
    let raw = parser.value()?;
    raw.to_string_lossy().parse::<DropTarget>()
}

/// One error text for `-d` given where no script will run (`serve` / `config`).
fn d_requires_script(role: &str) -> BoxError {
    format!("'-d' is not valid for '{role}'; it sets an ini directive for a script to run").into()
}

/// Reject cross-cutting script flags collected before a role that does not
/// accept them (currently `config`).
fn reject_script_flags(
    role: &str,
    ini: &[(String, String)],
    user: &Option<DropTarget>,
) -> Result<(), BoxError> {
    if !ini.is_empty() {
        return Err(d_requires_script(role));
    }
    if user.is_some() {
        return Err(format!("'--user' is not valid for '{role}'").into());
    }
    Ok(())
}

/// Continue after the `serve` keyword (or bare flags resolving to serve).
/// `--user` is valid here; `-d` is not.
fn finish_serve(
    parser: &mut lexopt::Parser,
    ini: Vec<(String, String)>,
    mut user: Option<DropTarget>,
) -> Result<Command, BoxError> {
    use lexopt::prelude::*;
    if !ini.is_empty() {
        return Err(d_requires_script("serve"));
    }
    while let Some(arg) = parser.next()? {
        match arg {
            // `--user=<name|name:group|uid|uid:gid>`: drop privileges to this
            // user after binding. Resolved eagerly so an unknown user/group
            // fails here, before any startup. Requires starting as root —
            // that is enforced at drop time, not parse time.
            Long("user") => user = Some(parse_user(parser)?),
            other => {
                return Err(
                    format!("unexpected argument to 'serve': {}", format_arg(&other)).into(),
                );
            }
        }
    }
    Ok(Command::Serve(ServeOptions { drop_to: user }))
}

/// Continue after the explicit `run` keyword. Accepts `-d`/`--user` before the
/// script path; the first positional is the script. Everything after the script
/// is the script's argv tail (raw passthrough — implicit `--`), so script flags
/// like `--verbose` reach PHP rather than lexopt.
fn finish_run(
    parser: &mut lexopt::Parser,
    mut ini: Vec<(String, String)>,
    mut user: Option<DropTarget>,
) -> Result<Command, BoxError> {
    use lexopt::prelude::*;
    while let Some(arg) = parser.next()? {
        match arg {
            Short('d') => ini.push(parse_ini_define(parser.value()?)),
            Long("user") => user = Some(parse_user(parser)?),
            Short('h') | Long("help") => {
                // `oxphp run --help` (before the script) prints help. A
                // `--help` *after* the script is the script's own argument.
                return Ok(Command::Help);
            }
            Value(script) => return build_run(script, parser, ini, user),
            other => {
                return Err(format!("unexpected argument to 'run': {}", format_arg(&other)).into());
            }
        }
    }

    Err("'run' requires a script path: oxphp run <script.php> [args…]".into())
}

/// Finalize a run role: `script` is the path; the raw tail is the script's
/// `$argv` (implicit `--`). `-` is reserved for a future stdin frontend.
fn build_run(
    script: OsString,
    parser: &mut lexopt::Parser,
    ini: Vec<(String, String)>,
    user: Option<DropTarget>,
) -> Result<Command, BoxError> {
    if script == "-" {
        return Err("reading the script from stdin ('-') is not yet supported".into());
    }
    let args = parser.raw_args()?.collect::<Vec<OsString>>();
    Ok(Command::Run(RunOptions {
        script: PathBuf::from(script),
        args,
        ini,
        user,
    }))
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
    oxphp serve [--user=<name|uid[:gid]>]
    oxphp [-d key=value]... [--user=<spec>] <script.php> [args]...

OPTIONS:
    -h, --help      Print this help and exit
    -v, --version   Print version information and exit

COMMANDS:
    serve           Start the HTTP server (default; same as bare 'oxphp')
    run             Execute a single PHP script under CLI semantics and exit
    config          Configuration utilities (see 'oxphp config --help')

SERVE:
    --user <spec>   Bind the listeners as the starting user (root, for ports
                    below 1024), then permanently drop to <spec> before any
                    traffic is served. <spec> is a user name, name:group, uid,
                    or uid:gid. Requires starting as root.

RUN:
    A bare script path runs it under the OxPHP engine — fibers, ox_shared, and
    the engine plugins are available — and exits with the script's code.
    'oxphp <script.php>' is shorthand for 'oxphp run <script.php>'. Any token
    that is not 'serve', 'run', or 'config' is treated as a script path.
    -d key=value    Set a php.ini directive for this run (repeatable)
    --user <spec>   Drop privileges to <spec> before running (requires root)
    Arguments after the script path are passed to PHP as $argv.

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
    fn terminal_flag_before_subcommand_wins() {
        // Terminal flags short-circuit: `--help` before a subcommand keyword
        // prints help rather than erroring.
        assert_eq!(
            parse_from(args(&["--help", "serve"])).unwrap(),
            Command::Help
        );
        assert_eq!(parse_from(args(&["--help", "run"])).unwrap(), Command::Help);
        assert_eq!(
            parse_from(args(&["--help", "config"])).unwrap(),
            Command::Help
        );
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

    fn run_opts(cmd: Command) -> RunOptions {
        match cmd {
            Command::Run(opts) => opts,
            other => panic!("expected Command::Run, got {other:?}"),
        }
    }

    #[test]
    fn run_subcommand_basic() {
        let opts = run_opts(parse_from(args(&["run", "hello.php"])).unwrap());
        assert_eq!(opts.script, PathBuf::from("hello.php"));
        assert!(opts.args.is_empty());
        assert!(opts.ini.is_empty());
        assert!(opts.user.is_none());
    }

    #[test]
    fn run_captures_script_args() {
        let opts = run_opts(parse_from(args(&["run", "s.php", "a", "b", "c"])).unwrap());
        assert_eq!(opts.script, PathBuf::from("s.php"));
        assert_eq!(opts.args, args(&["a", "b", "c"]));
    }

    #[test]
    fn run_script_flags_pass_through_to_php() {
        // Flags after the script path are the script's argv, not oxphp's —
        // an implicit `--` at the script positional. `--verbose` must reach PHP.
        let opts = run_opts(parse_from(args(&["run", "s.php", "--verbose", "-x"])).unwrap());
        assert_eq!(opts.args, args(&["--verbose", "-x"]));
    }

    #[test]
    fn run_d_ini_override_separate() {
        let opts =
            run_opts(parse_from(args(&["run", "-d", "memory_limit=512M", "s.php"])).unwrap());
        assert_eq!(
            opts.ini,
            vec![("memory_limit".to_string(), "512M".to_string())]
        );
        assert_eq!(opts.script, PathBuf::from("s.php"));
    }

    #[test]
    fn run_d_ini_override_attached() {
        let opts = run_opts(parse_from(args(&["run", "-dmemory_limit=512M", "s.php"])).unwrap());
        assert_eq!(
            opts.ini,
            vec![("memory_limit".to_string(), "512M".to_string())]
        );
    }

    #[test]
    fn run_d_repeatable() {
        let opts = run_opts(parse_from(args(&["run", "-d", "a=1", "-d", "b=2", "s.php"])).unwrap());
        assert_eq!(
            opts.ini,
            vec![
                ("a".to_string(), "1".to_string()),
                ("b".to_string(), "2".to_string())
            ]
        );
    }

    #[test]
    fn run_d_without_value_defaults_to_one() {
        // `php -d foo` sets foo=1.
        let opts = run_opts(parse_from(args(&["run", "-d", "opcache.enable", "s.php"])).unwrap());
        assert_eq!(
            opts.ini,
            vec![("opcache.enable".to_string(), "1".to_string())]
        );
    }

    #[test]
    fn run_d_after_script_is_script_arg() {
        // Everything after the script is raw passthrough — `-d` here is the
        // script's argument, not an oxphp ini override.
        let opts = run_opts(parse_from(args(&["run", "s.php", "-d", "x=y"])).unwrap());
        assert!(opts.ini.is_empty());
        assert_eq!(opts.args, args(&["-d", "x=y"]));
    }

    #[test]
    fn run_requires_a_script() {
        let err = parse_from(args(&["run"])).unwrap_err();
        assert!(err.to_string().contains("run"));
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
    fn bare_positional_is_implicit_run() {
        // A non-keyword first positional is a script path → implicit run.
        let opts = run_opts(parse_from(args(&["start"])).unwrap());
        assert_eq!(opts.script, PathBuf::from("start"));
        assert!(opts.args.is_empty());
        assert!(opts.ini.is_empty());
        assert!(opts.user.is_none());
    }

    #[test]
    fn implicit_run_php_with_args() {
        let opts = run_opts(parse_from(args(&["app.php", "a", "b", "--verbose"])).unwrap());
        assert_eq!(opts.script, PathBuf::from("app.php"));
        assert_eq!(opts.args, args(&["a", "b", "--verbose"]));
    }

    #[test]
    fn implicit_run_pathlike_no_extension() {
        // Extensionless path runs too — classification is keyword-vs-script,
        // never extension-based (php executes by file content).
        let opts = run_opts(parse_from(args(&["./bin/migrate"])).unwrap());
        assert_eq!(opts.script, PathBuf::from("./bin/migrate"));
    }

    #[test]
    fn implicit_run_with_d_before_script() {
        let opts = run_opts(parse_from(args(&["-d", "memory_limit=512M", "app.php"])).unwrap());
        assert_eq!(
            opts.ini,
            vec![("memory_limit".to_string(), "512M".to_string())]
        );
        assert_eq!(opts.script, PathBuf::from("app.php"));
    }

    #[test]
    fn keyword_match_is_exact() {
        // `serve.php` is not the `serve` keyword (exact match only) → implicit run.
        let opts = run_opts(parse_from(args(&["serve.php"])).unwrap());
        assert_eq!(opts.script, PathBuf::from("serve.php"));
    }

    #[test]
    fn d_without_script_is_error() {
        // `-d` sets an ini directive for a script; with no script (resolves to
        // serve) it is an error.
        let err = parse_from(args(&["-d", "memory_limit=512M"])).unwrap_err();
        assert!(err.to_string().contains("-d"));
    }

    #[test]
    fn stdin_dash_is_reserved() {
        let err = parse_from(args(&["-"])).unwrap_err();
        assert!(err.to_string().contains("stdin"));
    }

    #[test]
    fn first_terminal_flag_wins() {
        // No conflict detection any more: the first terminal flag short-circuits.
        assert_eq!(
            parse_from(args(&["--help", "--version"])).unwrap(),
            Command::Help
        );
        assert_eq!(
            parse_from(args(&["--version", "--help"])).unwrap(),
            Command::Version
        );
    }

    #[test]
    fn repeated_same_flag_is_ok() {
        assert_eq!(parse_from(args(&["-h", "--help"])).unwrap(), Command::Help);
    }
}

#[cfg(all(test, unix))]
mod privilege_drop_tests {
    use super::*;
    use std::ffi::{CString, OsString};

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    fn serve_opts(cmd: Command) -> ServeOptions {
        match cmd {
            Command::Serve(opts) => opts,
            other => panic!("expected Command::Serve, got {other:?}"),
        }
    }

    fn run_opts(cmd: Command) -> RunOptions {
        match cmd {
            Command::Run(opts) => opts,
            other => panic!("expected Command::Run, got {other:?}"),
        }
    }

    #[test]
    fn parses_numeric_uid_gid() {
        // High uid/gid with no passwd/group entry: host-independent. With no
        // passwd entry the user name resolves to None.
        let t: DropTarget = "1000001:1000002".parse().unwrap();
        assert_eq!(t.uid, 1000001);
        assert_eq!(t.gid, 1000002);
        assert_eq!(t.user, None);
    }

    #[test]
    fn bare_numeric_uid_with_passwd_resolves_gid_and_name() {
        // uid 0 always has a passwd entry (root); its primary gid is 0 on both
        // Linux and macOS, and the name reverse-resolves for initgroups().
        let t: DropTarget = "0".parse().unwrap();
        assert_eq!(t.uid, 0);
        assert_eq!(t.gid, 0);
        assert_eq!(t.user, Some(CString::new("root").unwrap()));
    }

    #[test]
    fn bare_numeric_uid_without_passwd_is_rejected() {
        // No passwd entry → no primary gid to default to → must be explicit.
        let err = "1000001".parse::<DropTarget>().unwrap_err();
        assert!(err.to_string().contains("group") || err.to_string().contains("uid:gid"));
    }

    #[test]
    fn resolves_user_name() {
        let t: DropTarget = "root".parse().unwrap();
        assert_eq!(t.uid, 0);
        assert_eq!(t.user, Some(CString::new("root").unwrap()));
    }

    #[test]
    fn resolves_name_with_numeric_group() {
        let t: DropTarget = "root:0".parse().unwrap();
        assert_eq!(t.uid, 0);
        assert_eq!(t.gid, 0);
    }

    #[test]
    fn rejects_empty() {
        assert!("".parse::<DropTarget>().is_err());
    }

    #[test]
    fn rejects_empty_user() {
        assert!(":5".parse::<DropTarget>().is_err());
    }

    #[test]
    fn rejects_empty_group() {
        assert!("root:".parse::<DropTarget>().is_err());
    }

    #[test]
    fn rejects_unknown_user() {
        assert!("oxphp-nonexistent-user-98765"
            .parse::<DropTarget>()
            .is_err());
    }

    #[test]
    fn rejects_unknown_group() {
        assert!("root:oxphp-nonexistent-group-98765"
            .parse::<DropTarget>()
            .is_err());
    }

    #[test]
    fn serve_user_flag_attached() {
        let opts = serve_opts(parse_from(args(&["serve", "--user=1000001:1000002"])).unwrap());
        let t = opts.drop_to.expect("drop_to should be set");
        assert_eq!(t.uid, 1000001);
        assert_eq!(t.gid, 1000002);
    }

    #[test]
    fn serve_user_flag_separate_value() {
        let opts = serve_opts(parse_from(args(&["serve", "--user", "1000001:1000002"])).unwrap());
        assert_eq!(opts.drop_to.expect("drop_to should be set").uid, 1000001);
    }

    #[test]
    fn serve_without_user_has_no_drop() {
        let opts = serve_opts(parse_from(args(&["serve"])).unwrap());
        assert!(opts.drop_to.is_none());
    }

    #[test]
    fn serve_user_rejects_bad_value() {
        let err = parse_from(args(&["serve", "--user=oxphp-nonexistent-user-98765"])).unwrap_err();
        assert!(err.to_string().contains("user") || err.to_string().contains("--user"));
    }

    #[test]
    fn implicit_run_user_before_script() {
        let opts = run_opts(parse_from(args(&["--user=1000001:1000002", "app.php"])).unwrap());
        let t = opts.user.expect("user should be set");
        assert_eq!(t.uid, 1000001);
        assert_eq!(t.gid, 1000002);
        assert_eq!(opts.script, PathBuf::from("app.php"));
    }

    #[test]
    fn run_keyword_user_before_script() {
        let opts =
            run_opts(parse_from(args(&["run", "--user=1000001:1000002", "app.php"])).unwrap());
        assert_eq!(opts.user.expect("user should be set").uid, 1000001);
    }

    #[test]
    fn config_rejects_collected_user() {
        let err = parse_from(args(&["--user=1000001:1000002", "config", "--check"])).unwrap_err();
        assert!(err.to_string().contains("--user") && err.to_string().contains("config"));
    }
}
