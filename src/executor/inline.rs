use std::cell::Cell;

use crate::php::bindings;
use crate::php::sapi;
use crate::types::{ScriptRequest, ScriptResponse};

use super::{ExecuteResult, ScriptExecutor};

thread_local! {
    static PHP_INITIALIZED: Cell<bool> = const { Cell::new(false) };
}

fn ensure_php_thread_init() {
    PHP_INITIALIZED.with(|init| {
        if !init.get() {
            unsafe {
                let _ = bindings::ts_resource_ex(0, std::ptr::null_mut());
            }
            init.set(true);
        }
    });
}

/// Manages PHP engine lifecycle for inline (spawn_blocking) execution.
///
/// PHP startup happens in `new()`, shutdown in `Drop`.
/// Each blocking thread lazily initializes its ZTS context on first use.
pub struct InlineExecutor;

impl InlineExecutor {
    pub fn new() -> Self {
        // 1. TSRM must be initialized first for ZTS builds
        if !unsafe { bindings::php_tsrm_startup() } {
            panic!("php_tsrm_startup() failed");
        }

        // 2. Build and register our SAPI module
        let mut module = sapi::build_sapi_module();
        unsafe {
            bindings::sapi_startup(&mut module);
        }

        // 3. Start the PHP engine
        let startup_result =
            unsafe { bindings::php_module_startup(&mut module, std::ptr::null_mut()) };
        if startup_result != 0 {
            panic!("php_module_startup() failed with code {startup_result}");
        }

        // 4. Install structured error logging callback (must be after php_module_startup)
        unsafe {
            sapi::install_error_cb();
        }

        tracing::info!("PHP engine initialized (inline mode)");
        Self
    }

    /// Execute a PHP script on the current thread.
    ///
    /// Must be called from a Tokio blocking thread (`spawn_blocking`).
    /// Lazily initializes PHP ZTS context on first call per thread.
    pub fn execute_inline(request: &ScriptRequest) -> ScriptResponse {
        ensure_php_thread_init();
        super::sapi::execute_request(request)
    }
}

impl ScriptExecutor for InlineExecutor {
    fn execute(&self, request: ScriptRequest) -> ExecuteResult {
        let response = Self::execute_inline(&request);
        ExecuteResult::Immediate(response)
    }

    fn shutdown(&self) {
        // MSHUTDOWN handled by Drop
    }
}

impl Drop for InlineExecutor {
    fn drop(&mut self) {
        unsafe {
            bindings::php_module_shutdown();
            bindings::sapi_shutdown();
            bindings::tsrm_shutdown();
        }
        tracing::info!("PHP engine shut down");
    }
}
