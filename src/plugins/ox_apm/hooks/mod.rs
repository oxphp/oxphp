//! APM hook infrastructure — registers internal PHP function hooks for
//! automatic span creation around database, HTTP client, cache, and I/O calls.
//!
//! ## Architecture
//!
//! 1. **Registration phase** (Rust, before PHP startup): `register_all()` calls
//!    each submodule's `register()` which calls `register_hook(class, func)`.
//!    This stores entries in the C bridge's pending hook list.
//!
//! 2. **Callback installation** (Rust, before PHP startup): `install_callbacks()`
//!    sets the before/after function pointers in the C bridge.
//!
//! 3. **Hook installation** (C, during MINIT): `oxphp_apm_install_registered_hooks()`
//!    looks up each pending function in Zend's tables and replaces its handler.
//!
//! 4. **Runtime** (C, during PHP execution): The wrapper calls before → original → after.
//!    The Rust callbacks create/close spans on the thread-local `ProfilingContext`.

pub mod curl;
pub mod file_io;
pub mod memcached;
pub mod mysqli;
pub mod pdo;
pub mod redis;

use std::cell::RefCell;
use std::sync::Arc;
use std::time::Instant;

use super::connection_meta::ConnectionMeta;

/// A frame pushed onto the thread-local stack by `before_callback` and
/// popped by `after_callback`. Carries state between the two calls.
#[derive(Debug)]
pub struct HookFrame {
    /// Span local ID returned by `ProfilingContext::push`.
    pub span_local_id: u32,
    /// Timestamp when the before callback fired (for precise timing).
    pub start: Instant,
    /// `true` for a query/statement-execution DB span — the only spans eligible
    /// for the `oxphp.db.slow` flag stamped in `after_callback`. A slow
    /// *connection* (`__construct`) is deliberately excluded: it is not a query,
    /// and its span duration already shows the latency.
    pub slow_eligible: bool,
}

thread_local! {
    /// Stack of active hook frames, mirroring the PHP call nesting.
    static HOOK_FRAMES: RefCell<Vec<HookFrame>> = const { RefCell::new(Vec::new()) };
}

/// Push a frame onto the thread-local hook frame stack.
pub fn push_frame(frame: HookFrame) {
    HOOK_FRAMES.with(|frames| frames.borrow_mut().push(frame));
}

/// Pop the most recent frame from the thread-local hook frame stack.
pub fn pop_frame() -> Option<HookFrame> {
    HOOK_FRAMES.with(|frames| frames.borrow_mut().pop())
}

// ---------------------------------------------------------------------------
// Database instrumentation (pure logic — unit-tested without PHP)
// ---------------------------------------------------------------------------

/// Upper bound on captured bind parameters, so a query with thousands of
/// parameters can't inflate a span attribute without limit.
const MAX_DB_PARAMS: usize = 64;

/// Span attribute key/value pairs (`Arc<str>` so static tags append cheaply).
type SpanAttrs = Vec<(Arc<str>, Arc<str>)>;

/// What a hooked database call is and how its arguments should be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DbAction {
    /// `PDO::__construct` — `args[0]` is the DSN string.
    PdoConstruct,
    /// `mysqli::__construct` — `args` are `(host, user, pass, db, port, …)`.
    MysqliConstruct,
    /// A statement-bearing call whose SQL is in `args[0]`: `query` / `exec` /
    /// `prepare`. The SQL is read and obfuscated directly from that call's own
    /// arguments, so the `db.statement` attribute is always the caller's own.
    Query,
    /// `PDOStatement`/`mysqli_stmt` `execute` — `$this` is the statement. Its
    /// SQL is not in the arguments; for `PDOStatement` it is recovered from the
    /// object's own `queryString` property, and `args[0]` may hold bound
    /// parameters. The span is timed for the slow-query flag.
    Execute,
}

/// Classify a hooked `(class, func)` pair as a database action, or `None` for
/// cache/HTTP/IO/other hooks (which get a bare span).
///
/// `prepare` is a `Query`: it reads its own `args[0]` SQL, so no cross-call
/// statement store is needed. That store was removed deliberately — keying it
/// by the raw PHP object handle was unsound, since PHP recycles handles from a
/// free list and an `execute` on a statement created by an un-hooked path (e.g.
/// `mysqli_stmt::prepare` via `stmt_init`, or a recycled handle) could read
/// another statement's SQL.
fn classify_db(class: &str, func: &str) -> Option<DbAction> {
    match (class, func) {
        ("PDO", "__construct") => Some(DbAction::PdoConstruct),
        ("mysqli", "__construct") => Some(DbAction::MysqliConstruct),
        ("PDO", "query") | ("PDO", "exec") | ("mysqli", "query") => Some(DbAction::Query),
        ("PDO", "prepare") | ("mysqli", "prepare") | ("mysqli_stmt", "prepare") => {
            Some(DbAction::Query)
        }
        ("PDOStatement", "execute") | ("mysqli_stmt", "execute") => Some(DbAction::Execute),
        _ => None,
    }
}

/// Append the OTel semantic-convention connection attributes to a span's
/// attribute list. Empty/zero fields are skipped so we never emit blank tags.
fn push_conn_attrs(attrs: &mut SpanAttrs, meta: &ConnectionMeta) {
    attrs.push((Arc::from("db.system"), Arc::from(meta.db_system)));
    if !meta.host.is_empty() {
        attrs.push((Arc::from("server.address"), Arc::from(meta.host.as_str())));
    }
    if meta.port != 0 {
        attrs.push((
            Arc::from("server.port"),
            Arc::from(meta.port.to_string().as_str()),
        ));
    }
    if !meta.database.is_empty() {
        attrs.push((Arc::from("db.name"), Arc::from(meta.database.as_str())));
    }
}

/// Build the `db.*` span attributes for a query given its raw SQL and optional
/// connection metadata: obfuscated `db.statement`, `db.operation`, plus the
/// connection attributes.
fn build_query_attributes(sql: &str, conn: Option<&ConnectionMeta>) -> SpanAttrs {
    let statement = crate::plugins::ox_otel::strip_nul(&super::sql::obfuscate(sql)).into_owned();
    let mut attributes: SpanAttrs = Vec::with_capacity(6);
    attributes.push((Arc::from("db.statement"), Arc::from(statement.as_str())));
    attributes.push((
        Arc::from("db.operation"),
        Arc::from(super::sql::extract_operation(sql)),
    ));
    if let Some(m) = conn {
        push_conn_attrs(&mut attributes, m);
    }
    attributes
}

// ---------------------------------------------------------------------------
// FFI bindings (only when compiling with PHP)
// ---------------------------------------------------------------------------

#[cfg(feature = "php")]
mod ffi {
    use std::os::raw::c_char;

    extern "C" {
        pub fn oxphp_apm_set_before(
            f: Option<
                unsafe extern "C" fn(
                    *const c_char,
                    *const c_char,
                    u32,
                    *mut std::ffi::c_void,
                    u32,
                    *mut std::ffi::c_void,
                ),
            >,
        );
        pub fn oxphp_apm_set_after(
            f: Option<
                unsafe extern "C" fn(
                    *const c_char,
                    *const c_char,
                    u32,
                    *mut std::ffi::c_void,
                    *mut std::ffi::c_void,
                ),
            >,
        );
        pub fn oxphp_apm_register_hook(class_name: *const c_char, func_name: *const c_char);
        pub fn oxphp_apm_hook_count_installed() -> i32;
        pub fn oxphp_apm_hook_count_approved() -> i32;
        pub fn oxphp_apm_unhook_all();
    }
}

// ---------------------------------------------------------------------------
// Registration (called from Rust before PHP startup)
// ---------------------------------------------------------------------------

/// Register a single internal PHP function for hooking.
///
/// `class_name` should be the PHP class name (e.g. "PDO") or empty string
/// for global functions. `func_name` is the method/function name.
///
/// This only records the intent — actual installation happens during MINIT.
#[cfg(feature = "php")]
pub fn register_hook(class_name: &str, func_name: &str) {
    use std::ffi::CString;
    let c_class = CString::new(class_name).unwrap_or_default();
    let c_func = CString::new(func_name).unwrap_or_default();
    unsafe {
        ffi::oxphp_apm_register_hook(c_class.as_ptr(), c_func.as_ptr());
    }
}

#[cfg(not(feature = "php"))]
pub fn register_hook(_class_name: &str, _func_name: &str) {
    // No-op on host without PHP
}

/// Register all hook targets across all submodules.
/// Returns the total number of functions registered for hooking.
pub fn register_all() -> usize {
    let mut count = 0;
    count += pdo::register();
    count += mysqli::register();
    count += curl::register();
    count += redis::register();
    count += memcached::register();
    count += file_io::register();
    count
}

/// Set the Rust before/after callbacks in the C bridge.
#[cfg(feature = "php")]
pub fn install_callbacks() {
    unsafe {
        ffi::oxphp_apm_set_before(Some(before_callback));
        ffi::oxphp_apm_set_after(Some(after_callback));
    }
}

#[cfg(not(feature = "php"))]
pub fn install_callbacks() {
    // No-op on host without PHP
}

/// Restore all hooks and clear callbacks.
#[cfg(feature = "php")]
pub fn unhook_all() {
    unsafe {
        ffi::oxphp_apm_unhook_all();
        ffi::oxphp_apm_set_before(None);
        ffi::oxphp_apm_set_after(None);
    }
}

#[cfg(not(feature = "php"))]
pub fn unhook_all() {}

/// Get count of installed hooks (for diagnostics).
#[cfg(feature = "php")]
pub fn hook_count() -> i32 {
    unsafe { ffi::oxphp_apm_hook_count_installed() }
}

#[cfg(not(feature = "php"))]
pub fn hook_count() -> i32 {
    0
}

/// Get count of approved hooks (global, available after MINIT).
#[cfg(feature = "php")]
pub fn approved_count() -> i32 {
    unsafe { ffi::oxphp_apm_hook_count_approved() }
}

#[cfg(not(feature = "php"))]
pub fn approved_count() -> i32 {
    0
}

// ---------------------------------------------------------------------------
// FFI argument readers (only when compiling with PHP)
// ---------------------------------------------------------------------------

/// Read `args[idx]` as a UTF-8 string, or `None` if the index is out of range
/// or the argument is not a (non-empty) PHP string. The `idx < argc` guard is
/// essential — `oxphp_arg_str` indexes into the raw zval array without a bound
/// check, so reading past `argc` is undefined behavior.
#[cfg(feature = "php")]
unsafe fn read_str_arg(args: *mut std::ffi::c_void, argc: u32, idx: u32) -> Option<String> {
    if idx >= argc {
        return None;
    }
    let mut len: usize = 0;
    let ptr = crate::bridge::ffi::oxphp_arg_str(args, idx, &mut len);
    if ptr.is_null() || len == 0 {
        return None;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    Some(String::from_utf8_lossy(slice).into_owned())
}

/// Read `args[idx]` as a PHP `int`, or `None` if out of range / not a long.
#[cfg(feature = "php")]
unsafe fn read_long_arg(args: *mut std::ffi::c_void, argc: u32, idx: u32) -> Option<i64> {
    if idx >= argc {
        return None;
    }
    // 3 == OXPHP_TYPE_LONG (see ext/bridge/oxphp_bridge.h).
    if crate::bridge::ffi::oxphp_val_arg_type(args, idx) != 3 {
        return None;
    }
    Some(crate::bridge::ffi::oxphp_arg_long(args, idx))
}

/// Best-effort `mysqli::__construct` connection metadata from positional args
/// `(host, user, pass, db, port, …)`. mysqli defaults the port to 3306; a lazy
/// or `mysqli_connect()`-style connection with no ctor args yields just the
/// system + default port.
#[cfg(feature = "php")]
unsafe fn mysqli_meta_from_args(args: *mut std::ffi::c_void, argc: u32) -> ConnectionMeta {
    let host = read_str_arg(args, argc, 0).unwrap_or_default();
    let database = read_str_arg(args, argc, 3).unwrap_or_default();
    let port = read_long_arg(args, argc, 4)
        .and_then(|p| u16::try_from(p).ok())
        .unwrap_or(3306);
    ConnectionMeta {
        db_system: "mysql",
        host,
        port,
        database,
    }
}

/// Foreach callback: append each bind parameter's stringified value to the
/// `Vec<String>` behind `user_data`, capped at [`MAX_DB_PARAMS`].
#[cfg(feature = "php")]
unsafe extern "C" fn params_iter_cb(
    _key_ptr: *const u8,
    _key_len: usize,
    _index: i64,
    val: *mut std::ffi::c_void,
    user_data: *mut std::ffi::c_void,
) {
    let out = &mut *(user_data as *mut Vec<String>);
    if out.len() >= MAX_DB_PARAMS {
        return;
    }
    out.push(format_param_value(val));
}

/// Stringify a single bind-parameter zval for the `db.params` attribute.
/// Non-scalars (array/object/resource) collapse to `?`.
#[cfg(feature = "php")]
unsafe fn format_param_value(val: *mut std::ffi::c_void) -> String {
    use crate::bridge::ffi;
    // Type codes mirror OXPHP_TYPE_* / ValType.
    match ffi::oxphp_val_type(val) {
        0 => "null".to_string(),
        1 => "false".to_string(),
        2 => "true".to_string(),
        3 => ffi::oxphp_val_long(val).to_string(),
        4 => ffi::oxphp_val_double(val).to_string(),
        5 => {
            let mut len: usize = 0;
            let ptr = ffi::oxphp_val_str(val, &mut len);
            if ptr.is_null() || len == 0 {
                String::new()
            } else {
                let slice = std::slice::from_raw_parts(ptr, len);
                String::from_utf8_lossy(slice).into_owned()
            }
        }
        _ => "?".to_string(),
    }
}

/// Collect bound parameters from `args[0]` (the params array passed to
/// `PDOStatement::execute`) into a `[v1, v2, …]` string, or `None` when there
/// is no array argument. Named-parameter keys are ignored (values only).
#[cfg(feature = "php")]
unsafe fn collect_params(args: *mut std::ffi::c_void, argc: u32) -> Option<String> {
    if argc == 0 {
        return None;
    }
    let arr = crate::bridge::ffi::oxphp_arg_array(args, 0);
    if arr.is_null() {
        return None;
    }
    let mut out: Vec<String> = Vec::new();
    crate::bridge::ffi::oxphp_array_foreach(
        arr,
        params_iter_cb,
        &mut out as *mut Vec<String> as *mut std::ffi::c_void,
    );
    if out.is_empty() {
        return None;
    }
    Some(format!("[{}]", out.join(", ")))
}

/// Read the `queryString` property of a statement object (`PDOStatement`), or
/// `None` when there is no such property (e.g. `mysqli_stmt`, which has none) or
/// no object receiver. This is the statement's own SQL, read from the object
/// itself — authoritative and immune to object-handle recycling, so no
/// cross-call store is involved. `queryString` is a plain declared property, so
/// the returned pointer is into the object's property table (stable for the
/// call), not a temporary — see `oxphp_object_read_property`'s contract.
#[cfg(feature = "php")]
unsafe fn read_object_query_string(this_zv: *mut std::ffi::c_void) -> Option<String> {
    if this_zv.is_null() {
        return None;
    }
    let prop = crate::bridge::ffi::oxphp_object_read_property(this_zv, c"queryString".as_ptr());
    if prop.is_null() {
        return None;
    }
    // `oxphp_val_str` returns NULL for a non-string (an unset property reads
    // back as the uninitialized/NULL zval), so a missing property → None.
    let mut len: usize = 0;
    let ptr = crate::bridge::ffi::oxphp_val_str(prop, &mut len);
    if ptr.is_null() || len == 0 {
        return None;
    }
    let slice = std::slice::from_raw_parts(ptr, len);
    Some(String::from_utf8_lossy(slice).into_owned())
}

// ---------------------------------------------------------------------------
// Rust callbacks invoked from C
// ---------------------------------------------------------------------------

/// Called before the original PHP internal function handler. Creates a child
/// span for the call and, for database hooks, decorates it with `db.*`
/// attributes read from the call arguments and connection metadata.
#[cfg(feature = "php")]
unsafe extern "C" fn before_callback(
    class_name: *const std::os::raw::c_char,
    func_name: *const std::os::raw::c_char,
    argc: u32,
    args: *mut std::ffi::c_void,
    this_handle: u32,
    this_zv: *mut std::ffi::c_void,
) {
    let cname = if class_name.is_null() {
        ""
    } else {
        unsafe { std::ffi::CStr::from_ptr(class_name) }
            .to_str()
            .unwrap_or("")
    };
    let fname = if func_name.is_null() {
        ""
    } else {
        unsafe { std::ffi::CStr::from_ptr(func_name) }
            .to_str()
            .unwrap_or("")
    };

    // Build span name: "ClassName::method" or just "function"
    let span_name = if cname.is_empty() {
        fname.to_string()
    } else {
        format!("{cname}::{fname}")
    };

    let start = Instant::now();

    // Every auto-hook span keeps the `source` tag; DB hooks add `db.*`.
    let mut attributes: Vec<(Arc<str>, Arc<str>)> =
        vec![(Arc::from("source"), Arc::from("auto-hook"))];
    let mut slow_eligible = false;

    if let Some(action) = classify_db(cname, fname) {
        // Only a query / statement execution counts for the slow-query flag —
        // a slow connection (`__construct`) is not a query.
        slow_eligible = matches!(action, DbAction::Query | DbAction::Execute);
        match action {
            DbAction::PdoConstruct => {
                if let Some(dsn) = read_str_arg(args, argc, 0) {
                    let meta = super::connection_meta::parse_pdo_dsn(&dsn);
                    push_conn_attrs(&mut attributes, &meta);
                    super::connection_meta::store(this_handle, meta);
                }
            }
            DbAction::MysqliConstruct => {
                let meta = mysqli_meta_from_args(args, argc);
                push_conn_attrs(&mut attributes, &meta);
                super::connection_meta::store(this_handle, meta);
            }
            DbAction::Query => {
                // `query` / `exec` / `prepare`: SQL is this call's own args[0],
                // so `db.statement` is always the caller's own — no store.
                if let Some(sql) = read_str_arg(args, argc, 0) {
                    let conn = super::connection_meta::get(this_handle);
                    attributes.extend(build_query_attributes(&sql, conn.as_ref()));
                }
            }
            DbAction::Execute => {
                // Recover the SQL from the statement object's own `queryString`
                // (PDOStatement) — read from the object itself, so a recycled
                // handle can never surface another statement's SQL. mysqli_stmt
                // has no such property, so its execute span carries no
                // db.statement (the SQL is on the mysqli prepare span).
                if let Some(sql) = read_object_query_string(this_zv) {
                    attributes.extend(build_query_attributes(&sql, None));
                }
                // Capture bound parameters when enabled. These are recorded raw
                // (not obfuscated) — the opt-in flag accepts PII in traces. The
                // span is timed for the slow-query flag via `slow_eligible`.
                if super::db_capture_params() {
                    if let Some(params) = collect_params(args, argc) {
                        attributes.push((
                            Arc::from("db.params"),
                            Arc::from(crate::plugins::ox_otel::strip_nul(&params).as_ref()),
                        ));
                    }
                }
            }
        }
    }

    let local_id = crate::profiling::PROFILING_CONTEXT
        .with(|stack| stack.borrow_mut().push(Arc::from(span_name), attributes));

    push_frame(HookFrame {
        span_local_id: local_id,
        start,
        slow_eligible,
    });
}

/// Called after the original PHP internal function handler. Stamps the
/// slow-query flag when a database call ran long, then closes the span.
#[cfg(feature = "php")]
unsafe extern "C" fn after_callback(
    _class_name: *const std::os::raw::c_char,
    _func_name: *const std::os::raw::c_char,
    _argc: u32,
    _args: *mut std::ffi::c_void,
    _return_value: *mut std::ffi::c_void,
) {
    let Some(frame) = pop_frame() else {
        return;
    };

    // Slow-query flag: stamp `oxphp.db.slow=true` before closing, when a
    // query/statement-execution span's wall-time met the threshold (0 disables).
    if frame.slow_eligible {
        let threshold = super::slow_query_ms();
        if threshold > 0 && frame.start.elapsed().as_millis() >= threshold as u128 {
            crate::profiling::PROFILING_CONTEXT.with(|stack| {
                if let Some(span) = stack.borrow_mut().get_mut(frame.span_local_id) {
                    span.attributes
                        .push((Arc::from("oxphp.db.slow"), Arc::from("true")));
                }
            });
        }
    }

    crate::profiling::PROFILING_CONTEXT.with(|stack| {
        stack.borrow_mut().pop(frame.span_local_id);
    });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal non-DB frame for the stack-mechanics tests.
    fn test_frame(span_local_id: u32) -> HookFrame {
        HookFrame {
            span_local_id,
            start: Instant::now(),
            slow_eligible: false,
        }
    }

    #[test]
    fn test_hook_frame_push_pop() {
        // Clear any leftover state from other tests
        HOOK_FRAMES.with(|frames| frames.borrow_mut().clear());

        push_frame(test_frame(1));
        push_frame(test_frame(2));

        // Pop should return in LIFO order
        let popped = pop_frame().expect("should have frame");
        assert_eq!(popped.span_local_id, 2);

        let popped = pop_frame().expect("should have frame");
        assert_eq!(popped.span_local_id, 1);
    }

    #[test]
    fn test_hook_frame_empty_pop() {
        // Clear any leftover state
        HOOK_FRAMES.with(|frames| frames.borrow_mut().clear());

        assert!(pop_frame().is_none());
    }

    #[test]
    fn test_hook_frame_nested_push_pop() {
        HOOK_FRAMES.with(|frames| frames.borrow_mut().clear());

        // Simulate nested hooks: PDO::query -> file_get_contents
        push_frame(test_frame(10));
        push_frame(test_frame(20));

        // Inner pops first
        let inner = pop_frame().unwrap();
        assert_eq!(inner.span_local_id, 20);

        // Outer pops second
        let outer = pop_frame().unwrap();
        assert_eq!(outer.span_local_id, 10);

        // Stack is empty
        assert!(pop_frame().is_none());
    }

    #[test]
    fn test_register_all_returns_count() {
        let count = register_all();
        // All submodules should register their functions
        // PDO: 5, mysqli: 5, curl: 4, redis: 10, memcached: 5, file_io: 5 = 34
        assert!(
            count > 0,
            "register_all should register at least some hooks"
        );
        assert_eq!(count, 34);
    }

    #[test]
    fn test_register_hook_no_php() {
        // On host without PHP, this should be a no-op (not panic)
        register_hook("PDO", "query");
        register_hook("", "file_get_contents");
    }

    // ── Database instrumentation (pure logic) ──

    #[test]
    fn test_classify_db() {
        assert_eq!(
            classify_db("PDO", "__construct"),
            Some(DbAction::PdoConstruct)
        );
        assert_eq!(
            classify_db("mysqli", "__construct"),
            Some(DbAction::MysqliConstruct)
        );
        assert_eq!(classify_db("PDO", "query"), Some(DbAction::Query));
        assert_eq!(classify_db("PDO", "exec"), Some(DbAction::Query));
        assert_eq!(classify_db("mysqli", "query"), Some(DbAction::Query));
        // prepare reads its own args[0] SQL — same handling as query, no store.
        assert_eq!(classify_db("PDO", "prepare"), Some(DbAction::Query));
        assert_eq!(classify_db("mysqli", "prepare"), Some(DbAction::Query));
        assert_eq!(classify_db("mysqli_stmt", "prepare"), Some(DbAction::Query));
        assert_eq!(
            classify_db("PDOStatement", "execute"),
            Some(DbAction::Execute)
        );
        assert_eq!(
            classify_db("mysqli_stmt", "execute"),
            Some(DbAction::Execute)
        );
        // Non-DB hooks are unclassified (they get a bare span).
        assert_eq!(classify_db("Redis", "get"), None);
        assert_eq!(classify_db("", "curl_exec"), None);
        assert_eq!(classify_db("", "file_get_contents"), None);
    }

    fn find<'a>(attrs: &'a [(Arc<str>, Arc<str>)], key: &str) -> Option<&'a str> {
        attrs
            .iter()
            .find(|(k, _)| k.as_ref() == key)
            .map(|(_, v)| v.as_ref())
    }

    #[test]
    fn test_build_query_attributes_obfuscates_and_extracts_operation() {
        let attrs = build_query_attributes("SELECT * FROM users WHERE email = 'a@b.com'", None);
        // PII is stripped from the statement, operation is the leading keyword.
        assert_eq!(
            find(&attrs, "db.statement"),
            Some("SELECT * FROM users WHERE email = ?")
        );
        assert_eq!(find(&attrs, "db.operation"), Some("SELECT"));
        // No connection metadata → no db.system / server.* tags.
        assert_eq!(find(&attrs, "db.system"), None);
    }

    #[test]
    fn test_build_query_attributes_with_connection_meta() {
        let conn = ConnectionMeta {
            db_system: "mysql",
            host: "db.internal".into(),
            port: 3306,
            database: "shop".into(),
        };
        let attrs = build_query_attributes("INSERT INTO t VALUES (1)", Some(&conn));
        assert_eq!(find(&attrs, "db.operation"), Some("INSERT"));
        assert_eq!(find(&attrs, "db.system"), Some("mysql"));
        assert_eq!(find(&attrs, "server.address"), Some("db.internal"));
        assert_eq!(find(&attrs, "server.port"), Some("3306"));
        assert_eq!(find(&attrs, "db.name"), Some("shop"));
    }

    #[test]
    fn test_push_conn_attrs_skips_empty_fields() {
        // sqlite: no host, no port — only db.system and the file path as db.name.
        let conn = ConnectionMeta {
            db_system: "sqlite",
            host: String::new(),
            port: 0,
            database: "/tmp/app.sqlite".into(),
        };
        let mut attrs = Vec::new();
        push_conn_attrs(&mut attrs, &conn);
        assert_eq!(find(&attrs, "db.system"), Some("sqlite"));
        assert_eq!(find(&attrs, "db.name"), Some("/tmp/app.sqlite"));
        assert_eq!(find(&attrs, "server.address"), None);
        assert_eq!(find(&attrs, "server.port"), None);
    }
}
