//! One-shot CLI execution: `oxphp run <script.php> [args…]`.
//!
//! Executes a single PHP file to completion under CLI semantics and exits with
//! the script's exit code. Everything runs on the main thread — there is no
//! HTTP listener, no accept loop, and no resident worker pool. What sets this
//! apart from a bare `php file.php` is that the OxPHP engine is available
//! underneath: fibers (`oxphp_sleep`), `ox_shared`, and the engine plugins
//! (full `oxphp_async()` dispatch additionally needs `ASYNC_WORKERS>0`).
//!
//! The path is self-contained — it builds its own lightweight engine and
//! returns an exit code — so it never touches the HTTP startup in `main.rs`.

use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::sync::Arc;

use crate::cli::RunOptions;
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
    let openable = std::fs::File::open(&opts.script)
        .ok()
        .and_then(|f| f.metadata().ok())
        .is_some_and(|m| m.is_file());
    if !openable {
        eprintln!(
            "oxphp: Could not open input file: {}",
            opts.script.display()
        );
        return 1;
    }

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

    let config = crate::config::Config::from_env().ok();

    // ── Superglobals are always on for the CLI role, regardless of the
    //    SUPERGLOBALS_ENABLED config (an HTTP per-request perf toggle). A
    //    one-shot script needs $argv / $_SERVER / $_ENV to be a useful CLI, so
    //    `set_cli_request_data` forces `sg_enabled = true` on the request side;
    //    we force it on the bridge side here to match (they must agree). ──

    // ── Register plugin functions / classes / decorators BEFORE MINIT so the
    //    PHP module startup can expose them to the compiler (OPcache needs them
    //    at compile time). Mirrors the serve path's pre-startup wiring, minus
    //    the HTTP request accessors. ──
    unsafe {
        bindings::oxphp_bridge_set_superglobals_enabled(true);
    }

    let native_fns = plugin_manager.take_native_php_functions();
    if !native_fns.is_empty() {
        sapi::register_native_plugin_functions(native_fns);
    }
    let php_defs = plugin_manager.take_php_definitions();
    if !php_defs.classes.is_empty()
        || !php_defs.interfaces.is_empty()
        || !php_defs.enums.is_empty()
        || !php_defs.attributes.is_empty()
        || !php_defs.functions.is_empty()
    {
        sapi::register_php_definitions(php_defs);
    }
    let registry = Arc::new(crate::decorator::DecoratorRegistry::new());
    for def in plugin_manager.take_decorators() {
        registry.register_rust(Arc::from(def.decorator));
    }
    crate::decorator::dispatch::install_bridge_callbacks(Arc::clone(&registry));

    // ── PHP engine startup with the CLI SAPI personality. ──
    unsafe {
        if !bindings::php_tsrm_startup() {
            eprintln!("oxphp: php_tsrm_startup() failed");
            return 1;
        }
        let mut module = sapi::build_cli_sapi_module(ini_ptr);
        bindings::sapi_startup(&mut module);
        // sapi_startup() sets sapi_module.ini_entries = NULL (main/SAPI.c). php-cli
        // re-attaches the pointer right here; php_module_startup() then copies the
        // struct into the global again, so php_init_config() actually parses the
        // blob. Without this the defaults + -d are silently discarded and
        // max_execution_time falls back to the engine default 30s, killing any
        // long-running `oxphp run` (migration, daemon, async loop) via SIGALRM.
        module.ini_entries = ini_ptr as *mut c_char;
        let rc = bindings::php_module_startup(&mut module, std::ptr::null_mut());
        if rc != 0 {
            eprintln!("oxphp: php_module_startup() failed (code {rc})");
            return 1;
        }
        sapi::install_error_cb();

        // Bridge TLS for this (main) thread so SG()/EG() resolve correctly
        // from liboxphp_bridge.so, then mark it as worker 0.
        bindings::ts_resource_ex(0, std::ptr::null_mut());
        bindings::oxphp_bridge_tsrm_update();
        bindings::oxphp_bridge_init_ctx();
        bindings::oxphp_bridge_set_worker_id(0);
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
    let async_workers = config.as_ref().map(|c| c.async_workers).unwrap_or(0);
    let async_queue = config.as_ref().map(|c| c.async_queue_capacity).unwrap_or(0);
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
/// `php_request_startup` → define std streams → `php_execute_script` →
/// `php_request_shutdown`, propagating the exit code. CLI ini defaults and the
/// `-d` overrides are not applied here — they live in the SAPI `ini_entries`
/// blob (see `build_ini_entries`), parsed at config stage during startup.
fn run_cli_oneshot(script: &Path, args: &[std::ffi::OsString]) -> i32 {
    // Stage $_SERVER (PHP_SELF, SCRIPT_*, environment).
    sapi::set_cli_request_data(script);

    // Build argv = [script, args…] as C strings. PHP's php_build_argv() copies
    // these during php_request_startup (register_argc_argv defaults on), but
    // the array must stay valid across that call.
    let mut argv_cstrings: Vec<CString> = Vec::with_capacity(args.len() + 1);
    argv_cstrings.push(cstring_from_bytes(script.as_os_str().as_bytes()));
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

    if unsafe { bindings::php_request_startup() } != 0 {
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

    let script_c = cstring_from_bytes(script.as_os_str().as_bytes());
    let mut file_handle: bindings::zend_file_handle = unsafe { std::mem::zeroed() };
    unsafe {
        bindings::zend_stream_init_filename(&mut file_handle, script_c.as_ptr());
    }
    file_handle.primary_script = true;

    // oxphp_execute_script_safe wraps php_execute_script in zend_try only as a
    // last-resort guard against a stray bailout escaping FFI. In PHP 8.4+ that
    // is effectively unreachable: exit()/die() is an ordinary function that does
    // a graceful unwind (zend_throw_unwind_exit, not zend_bailout), and
    // php_execute_script_ex catches its own bailouts — including the uncaught-
    // exception path — and returns normally. So the return value is ignored.
    unsafe {
        bindings::oxphp_execute_script_safe(&mut file_handle as *mut _ as *mut c_void);
    }
    // EG(exit_status) is the single source of truth, exactly like php-cli's
    // `return EG(exit_status)`: exit($code)/die($code) store it directly; a fatal
    // error, uncaught exception, or parse error makes the engine set it to 255
    // (main/main.c, Zend/zend.c); a clean run leaves it 0. Read before
    // php_request_shutdown clears engine state.
    let exit_status = unsafe { bindings::oxphp_bridge_get_exit_status() };

    unsafe {
        bindings::zend_destroy_file_handle(&mut file_handle);
        bindings::php_request_shutdown(std::ptr::null_mut());
    }
    sapi::clear_request_data();

    // Detach SG(request_info).argc/argv from the storage we are about to free
    // (clear_request_data only nulls request_method/query/etc., not argv) so
    // the engine globals never dangle. Defensive — the process exits next.
    unsafe {
        bindings::oxphp_bridge_set_cli_args(0, std::ptr::null_mut());
    }
    drop(argv_ptrs);
    drop(argv_cstrings);

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
         register_argc_argv=1\n\
         implicit_flush=1\n\
         output_buffering=0\n\
         max_execution_time=0\n\
         max_input_time=-1\n\
         display_errors=stderr\n",
    );
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
