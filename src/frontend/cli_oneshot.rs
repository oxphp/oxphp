//! One-shot CLI execution: `oxphp run <script.php> [args…]`.
//!
//! Executes a single PHP file (or `-r` code, or a script on standard input) to
//! completion under CLI semantics and exits with the script's exit code.
//! Everything runs on the main thread — there is no HTTP listener, no accept
//! loop, and no resident worker pool. What sets this
//! apart from a bare `php file.php` is that the OxPHP engine is available
//! underneath: fibers (`oxphp_sleep`), `ox_shared`, and the engine plugins
//! (full `oxphp_async()` dispatch additionally needs `ASYNC_WORKERS>0`).
//!
//! The path is self-contained — it builds its own lightweight engine and
//! returns an exit code — so it never touches the HTTP startup in `main.rs`.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;

use crate::cli::{RunOptions, Script};
use crate::php::{bindings, sapi};

/// Execute `opts.script` under the CLI personality and return the process
/// exit code. Builds the engine (MINIT with a `"cli"` SAPI, engine-only
/// plugin profile, a small Tokio runtime for async), runs the script, then
/// tears everything down.
pub fn run(opts: RunOptions) -> i32 {
    // ── Fail fast on an unopenable script, matching php-cli: it prints
    //    "Could not open input file: <path>" and exits 1, rather than spinning
    //    up the engine only to die with E_COMPILE_ERROR (exit 255). Open the
    //    file (don't just stat it) so a permission error is caught too, then
    //    reject anything that isn't a regular file (a directory opens fine). ──
    if let Script::File(path) = &opts.script {
        let openable = std::fs::File::open(path)
            .ok()
            .and_then(|f| f.metadata().ok())
            .is_some_and(|m| m.is_file());
        if !openable {
            eprintln!("oxphp: Could not open input file: {}", path.display());
            return 1;
        }
    }

    // ── A bare `oxphp` this script starts through `PHP_BINARY` checks this
    //    against its parent pid to read its script from standard input
    //    (`cli::parse`). Set before plugin init: no other thread exists yet,
    //    and `set_var` races with any thread reading the environment. ──
    std::env::set_var(crate::cli::RUN_PID_ENV, std::process::id().to_string());

    // ── ini: php-cli's hardcoded defaults + the user's `-d` overrides, folded
    //    into the SAPI `ini_entries` blob (php-cli parity). Leaked for the
    //    process lifetime because `php_module_startup` keeps the pointer. NOTE:
    //    `sapi_startup` nulls the pointer, so it must be re-attached afterwards
    //    (see below) or the whole blob is silently dropped. ──
    let ini_cstring = match build_ini_entries(&opts.ini) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("oxphp: {e}");
            return 2;
        }
    };
    let ini_ptr: *const c_char = ini_cstring.into_raw();

    // ── `PHP_BINARY` is resolved at module startup from this path, so a library
    //    that starts PHP workers through it (amphp/parallel) starts `oxphp`.
    //    Not argv[0] as php-cli passes: a bare name is looked up on PATH, which
    //    may find a different binary. Leaked like `ini_ptr`; null if the path
    //    cannot be read leaves `PHP_BINARY` empty. ──
    let exe_ptr: *mut c_char = std::env::current_exe()
        .ok()
        .and_then(|p| CString::new(p.as_os_str().as_bytes()).ok())
        .map_or(std::ptr::null_mut(), CString::into_raw);

    // ── `-c`: php.ini looked up in this path instead of the default search;
    //    leaked like `ini_ptr`. ──
    let ini_path_ptr: *mut c_char = opts.ini_path.as_ref().map_or(std::ptr::null_mut(), |p| {
        cstring_from_bytes(p.as_bytes()).into_raw()
    });

    // ── Engine-only plugin profile. The HTTP-frontend features (rate limit,
    //    access log, static files, metrics endpoint, …) are not PluginManager
    //    plugins — they are event handlers registered in the serve path, which
    //    this role never runs, so they are excluded for free. ──
    let mut dispatcher = crate::events::EventDispatcher::new();
    let mut plugin_manager = crate::plugin::PluginManager::new();
    #[cfg(feature = "plugin-async")]
    plugin_manager.add(Box::new(crate::plugins::ox_async::AsyncPlugin::new()));
    #[cfg(feature = "plugin-shared")]
    plugin_manager.add(Box::new(crate::plugins::ox_shared::SharedPlugin::default()));
    if let Err(e) = plugin_manager.init_all(&mut dispatcher) {
        eprintln!("oxphp: plugin initialization failed: {e}");
        return 1;
    }

    // ── `run` is a one-shot script executor: it parses only the env vars it
    //    actually consumes (`SUPERGLOBALS_ENABLED` and the `ASYNC_*` pool
    //    settings) instead of the full server config. A job container that
    //    inherits a web deployment's env template must not fail on
    //    server-only variables it never uses (an `ENTRY_FILE` pointing into
    //    an unmounted docroot, a bad `TLS_MIN_VERSION`, …) — but garbage in a
    //    variable this path DOES honor still fails loudly instead of
    //    silently re-enabling superglobals an operator disabled (folding the
    //    whole process env, secrets included, into $_SERVER) or turning off
    //    the async pool. ──
    //
    // ── Honor SUPERGLOBALS_ENABLED on the CLI path too (defaults to `true`).
    //    When an operator sets it `false`, the process-environment fold into
    //    $_SERVER is skipped, matching the HTTP worker. The $_SERVER script
    //    skeleton (PHP_SELF / SCRIPT_* / DOCUMENT_ROOT) and $argv / $argc are
    //    unconditional, so a one-shot script stays a usable CLI either way.
    //    `set_cli_request_data` reads the same flag back from the bridge, so
    //    both sides always agree. ──
    let superglobals_enabled = match crate::config::parse_env_bool("SUPERGLOBALS_ENABLED", true) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("oxphp: invalid configuration: {e}");
            return 1;
        }
    };

    // ── Register plugin functions / classes / decorators BEFORE MINIT so the
    //    PHP module startup can expose them to the compiler (OPcache needs them
    //    at compile time). Shared with the serve path so a new plugin-artifact
    //    kind can never reach only one frontend. This also stores the
    //    superglobals flag in the bridge (read during MINIT and request setup).
    //    The HTTP request accessors are intentionally omitted. ──
    let _registry =
        crate::php::bootstrap::register_plugin_artifacts(&mut plugin_manager, superglobals_enabled);

    // ── PHP engine startup with the CLI SAPI personality. ──
    // SAFETY: the only engine startup in this process, on the main thread, before
    // any PHP thread exists; `ini_ptr`, `exe_ptr` and `ini_path_ptr` are leaked,
    // so the pointers `sapi_module` keeps stay valid for the process lifetime.
    unsafe {
        if !bindings::php_tsrm_startup() {
            eprintln!("oxphp: php_tsrm_startup() failed");
            return 1;
        }
        let mut module = sapi::build_cli_sapi_module(ini_ptr, exe_ptr);
        // `-n` / `-c`, as php-cli sets them; sapi_startup() keeps both.
        module.php_ini_ignore = c_int::from(opts.ini_ignore);
        module.php_ini_path_override = ini_path_ptr;
        bindings::sapi_startup(&mut module);
        // sapi_startup() sets sapi_module.ini_entries = NULL (main/SAPI.c). php-cli
        // re-attaches the pointer right here; php_module_startup() then copies the
        // struct into the global again, so php_init_config() actually parses the
        // blob. Without this the defaults + -d are silently discarded and
        // max_execution_time falls back to the engine default 30s, killing any
        // long-running `oxphp run` (migration, daemon, async loop) via the execution timer.
        module.ini_entries = ini_ptr as *mut c_char;
        let rc = bindings::php_module_startup(&mut module, std::ptr::null_mut());
        if rc != 0 {
            eprintln!("oxphp: php_module_startup() failed (code {rc})");
            return 1;
        }
        sapi::install_error_cb();

        // Bridge TLS for this (main) thread so SG()/EG() resolve correctly
        // from liboxphp_bridge.so, then mark it as worker 0.
        crate::php::thread::attach(0);
    }
    crate::php::worker_registry::init_workers(1);

    // ── Tokio runtime, built only when an async worker pool exists. Fibers
    //    (`oxphp_sleep`) do NOT need it — their timer is an Instant-based poll
    //    (`src/php/fiber.rs`). The only Tokio consumer is the async pool, whose
    //    own OS threads `block_on` the cloned handle; a multi-thread runtime
    //    with one worker (not current_thread) gives them an always-on driver
    //    thread independent of which pool thread is currently inside `block_on`.
    //    With `ASYNC_WORKERS=0` (the `oxphp run` default) no pool is created, so
    //    no runtime — and no extra OS thread — is spawned. ──
    let (async_workers, async_queue, async_max_fibers) =
        match crate::config::resolve_async_pool_env() {
            Ok(triple) => triple,
            Err(e) => {
                eprintln!("oxphp: invalid configuration: {e}");
                return 1;
            }
        };
    let mut async_pool =
        crate::executor::async_pool::AsyncWorkerPool::new(async_workers, async_queue, None);

    let runtime = if async_pool.is_some() {
        match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
        {
            Ok(rt) => Some(rt),
            Err(e) => {
                eprintln!("oxphp: failed to build Tokio runtime: {e}");
                return 1;
            }
        }
    } else {
        None
    };

    if let (Some(pool), Some(rt)) = (async_pool.as_mut(), runtime.as_ref()) {
        pool.start();
        sapi::set_global_async_tx(pool.task_sender());
        let inflight_cap = async_max_fibers.saturating_mul(pool.worker_count());
        sapi::set_global_async_inflight(std::sync::Arc::new(
            crate::executor::async_fiber::InFlightCounter::new(inflight_cap),
        ));
        sapi::set_async_tokio_handle(rt.handle().clone());
        sapi::register_async_callbacks();
    }
    sapi::register_fiber_callbacks();

    // ── Run the script. ──
    let code = run_cli_oneshot(&opts.script, &opts.args);

    // ── Teardown. We stop the async pool and plugins, then drop the runtime,
    //    but deliberately skip php_module_shutdown / sapi_shutdown /
    //    tsrm_shutdown: the process exits immediately after returning, so the
    //    OS reclaims everything and a full engine teardown only adds latency.
    //    Caveat: if a future build enables `opcache.file_cache`, the cache is
    //    written during MSHUTDOWN — that would need an explicit shutdown here. ──
    if let Some(pool) = async_pool.as_mut() {
        pool.shutdown();
    }
    plugin_manager.shutdown_all();
    drop(runtime);

    code
}

/// The one-shot request lifecycle: stage `$_SERVER`/`$argv`, then
/// `php_request_startup` → define std streams → execute the script or code →
/// `php_request_shutdown`, propagating the exit code. CLI ini defaults and the
/// `-d` overrides are not applied here — they live in the SAPI `ini_entries`
/// blob (see `build_ini_entries`), parsed at config stage during startup.
fn run_cli_oneshot(script: &Script, args: &[std::ffi::OsString]) -> i32 {
    // php-cli's names: code that is not a file is "Standard input code" in
    // `$argv[0]` and PHP_SELF, and its SCRIPT_FILENAME is empty.
    let (php_self, script_filename) = match script {
        Script::File(path) => (path.as_os_str(), path.as_os_str()),
        Script::Code(_) | Script::Stdin => ("Standard input code".as_ref(), "".as_ref()),
    };

    // Stage $_SERVER (PHP_SELF, SCRIPT_*, environment).
    sapi::set_cli_request_data(
        &php_self.to_string_lossy(),
        &script_filename.to_string_lossy(),
    );

    // Build argv = [php_self, args…] as C strings. PHP's php_build_argv() copies
    // these during php_request_startup, but the array must stay valid across
    // that call.
    let mut argv_cstrings: Vec<CString> = Vec::with_capacity(args.len() + 1);
    argv_cstrings.push(cstring_from_bytes(php_self.as_bytes()));
    for arg in args {
        argv_cstrings.push(cstring_from_bytes(arg.as_bytes()));
    }
    let mut argv_ptrs: Vec<*mut c_char> = argv_cstrings
        .iter()
        .map(|c| c.as_ptr() as *mut c_char)
        .collect();
    unsafe {
        bindings::oxphp_bridge_set_cli_args(argv_ptrs.len() as c_int, argv_ptrs.as_mut_ptr());
    }

    // request_time must be set before php_request_startup — OPcache's RINIT
    // reads it (and a 0 value breaks its file-update protection check).
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    unsafe {
        bindings::oxphp_bridge_set_request_time(now);
    }

    // The script is one request, and the process shuts down after it.
    if !unsafe { crate::php::thread::open_request(crate::plugin::RequestScope::OneRequest) } {
        eprintln!("oxphp: php_request_startup() failed");
        return 1;
    }

    // Define STDIN / STDOUT / STDERR for php-cli parity (composer, artisan and
    // friends expect them). Guarded, so harmless if a script defines its own.
    let bootstrap = c"defined('STDIN')||define('STDIN',fopen('php://stdin','rb'));\
defined('STDOUT')||define('STDOUT',fopen('php://stdout','wb'));\
defined('STDERR')||define('STDERR',fopen('php://stderr','wb'));";
    unsafe {
        bindings::oxphp_bridge_eval(bootstrap.as_ptr());
    }

    // php-cli parity: skip a leading `#!` shebang line when compiling the file
    // (CG(skip_shebang)=1, sapi/cli/php_cli.c). Set immediately before the
    // compile — robust regardless of what the bootstrap block above compiled —
    // otherwise the shebang line is emitted as inline text and leaks into the
    // script's output, breaking `#!/usr/bin/env oxphp` extensionless scripts.
    //
    // Also php-cli parity: keep the launch directory as the working directory.
    // Otherwise php_execute_script changes into the script's directory first and
    // resolves the relative name for `__FILE__` from there: `bin/bin/x.php`.
    // SAFETY: on the main thread inside the request opened above; both calls
    // only set a flag in this thread's engine globals.
    unsafe {
        bindings::oxphp_bridge_skip_shebang();
        bindings::oxphp_bridge_set_no_chdir();
    }

    let exit_status = match script {
        Script::Code(code) => {
            let code_c = cstring_from_bytes(code.as_bytes());
            // SAFETY: inside the request opened above; `code_c` outlives the call.
            unsafe { bindings::oxphp_bridge_cli_eval_code(code_c.as_ptr()) };
            // SAFETY: reads this thread's engine globals before the request closes.
            unsafe { bindings::oxphp_bridge_get_exit_status() }
        }
        Script::File(path) => execute_primary_script(&path.to_string_lossy(), |handle| {
            // php-cli parity: open the file by its path, so it is not looked up
            // in include_path and open_basedir does not apply to it. The open can
            // fail where the check in `run` passed: like a ZTS php-cli, it folds
            // `..` before following a symlink, so `link/../x.php` is not the file
            // the kernel opens. (VCWD_FOPEN is virtual_fopen only under ZTS; an
            // NTS php uses fopen.)
            let path_c = cstring_from_bytes(path.as_os_str().as_bytes());
            // SAFETY: inside the request opened above; the handle copies the
            // name, so `path_c` only has to outlive the call.
            unsafe { bindings::oxphp_bridge_cli_open_script(handle, path_c.as_ptr()) }
        }),
        Script::Stdin => execute_primary_script("Standard input code", |handle| {
            // SAFETY: inside the request opened above.
            unsafe { bindings::oxphp_bridge_cli_open_stdin(handle) }
        }),
    };

    // SAFETY: on the main thread, closing the request opened above.
    unsafe { crate::php::thread::close_request() };
    sapi::clear_request_data();

    // Detach SG(request_info).argc/argv from the storage we are about to free
    // (clear_request_data only nulls request_method/query/etc., not argv) so
    // the engine globals never dangle. Defensive — the process exits next.
    // SAFETY: on the main thread, after the request closed; only stores the
    // null pair in this thread's SG(request_info).
    unsafe {
        bindings::oxphp_bridge_set_cli_args(0, std::ptr::null_mut());
    }
    drop(argv_ptrs);
    drop(argv_cstrings);

    exit_status
}

/// Open the primary script with `open` (a bridge opener: 0 on success) and run
/// it, returning its exit code; 1 with php-cli's message when it cannot be
/// opened. Call inside the request.
fn execute_primary_script(name: &str, open: impl FnOnce(*mut c_void) -> c_int) -> i32 {
    // SAFETY: an all-zero zend_file_handle is the unset state the bridge
    // openers initialise and zend_destroy_file_handle accepts.
    let mut file_handle: bindings::zend_file_handle = unsafe { std::mem::zeroed() };
    let opened = open(&mut file_handle as *mut _ as *mut c_void) == 0;
    file_handle.primary_script = true;

    let exit_status = if opened {
        // oxphp_execute_script_safe wraps php_execute_script in zend_try only as a
        // last-resort guard against a stray bailout escaping FFI. In PHP 8.4+ that
        // is effectively unreachable: exit()/die() is an ordinary function that does
        // a graceful unwind (zend_throw_unwind_exit, not zend_bailout), and
        // php_execute_script_ex catches its own bailouts — including the uncaught-
        // exception path — and returns normally. So the return value is ignored.
        // SAFETY: inside the request, with a handle the bridge initialised from
        // an open file.
        unsafe {
            bindings::oxphp_execute_script_safe(&mut file_handle as *mut _ as *mut c_void);
        }
        // EG(exit_status) is the single source of truth, exactly like php-cli's
        // `return EG(exit_status)`: exit($code)/die($code) store it directly; a fatal
        // error, uncaught exception, or parse error makes the engine set it to 255
        // (main/main.c, Zend/zend.c); a clean run leaves it 0. Read before
        // php_request_shutdown clears engine state.
        // SAFETY: reads this thread's engine globals before the request closes.
        unsafe { bindings::oxphp_bridge_get_exit_status() }
    } else {
        eprintln!("oxphp: Could not open input file: {name}");
        1
    };

    // SAFETY: the handle is zeroed or initialised by the opener, and destroyed
    // once, inside the request.
    unsafe { bindings::zend_destroy_file_handle(&mut file_handle) };
    exit_status
}

/// Build the SAPI `ini_entries` blob: php-cli's hardcoded defaults followed by
/// the user's `-d key=value` overrides. `ini_entries` is parsed *after* php.ini
/// in `php_init_config` (main/php_ini.c), so these win over any php.ini/conf.d —
/// this is the mechanism behind `php -d`. Being applied at config stage (not via
/// a runtime `zend_alter_ini`), they take effect for *every* directive type,
/// including PHP_INI_SYSTEM/PERDIR ones (`opcache.*`, `register_argc_argv`, …)
/// that a `ZEND_INI_USER` runtime alteration would silently refuse.
///
/// `-d` keys/values are validated to contain no newline so a crafted value
/// cannot inject an extra directive line into the blob.
fn build_ini_entries(ini: &[(String, String)]) -> Result<CString, String> {
    let mut blob = String::from(
        "html_errors=0\n\
         implicit_flush=1\n\
         output_buffering=0\n\
         max_execution_time=0\n\
         max_input_time=-1\n\
         display_errors=stderr\n",
    );
    // 8.4 builds `$argv` only while it is on, and php-cli 8.4 forces it on over a
    // php.ini that turns it off (both shipped templates do). 8.5 builds `$argv`
    // from argc without it and php-cli 8.5 no longer sets it; with it on, each
    // async pool thread (argc 0) would raise E_DEPRECATED for deriving argv from
    // the query string as its request starts.
    #[cfg(php_v8_4)]
    blob.push_str("register_argc_argv=1\n");
    for (key, value) in ini {
        if key.is_empty() || key.contains(['\n', '\r', '=']) {
            return Err(format!("invalid -d key {key:?}"));
        }
        if value.contains(['\n', '\r']) {
            return Err(format!("-d {key}: value may not contain a newline"));
        }
        blob.push_str(key);
        blob.push('=');
        blob.push_str(value);
        blob.push('\n');
    }
    CString::new(blob).map_err(|_| "ini contains an interior NUL byte".to_string())
}

/// Build a `CString` from raw bytes, truncating at an interior NUL rather than
/// failing — paths/args with NUL bytes are not representable in PHP anyway.
/// The truncated slice is NUL-free, so `CString::new` cannot fail here.
fn cstring_from_bytes(bytes: &[u8]) -> CString {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    CString::new(&bytes[..end]).unwrap_or_default()
}
