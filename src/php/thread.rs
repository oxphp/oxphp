//! The protocol by which the server's threads run PHP — attach to PHP, open
//! a PHP request, close it — and the points at which plugins see it
//! (`crate::plugin::php_thread`).

use crate::php::bindings;
use crate::plugin::php_thread::{installed, RequestScope};

/// Attach the calling thread to PHP: its TSRM resources, the bridge
/// library's view of them, and the bridge's per-thread context, marked with
/// `worker_id`.
///
/// # Safety
/// PHP's module startup has run. A thread PHP has attached already — the
/// main thread of `oxphp run`, which ran that startup — gets back the
/// resources it has.
pub(crate) unsafe fn attach(worker_id: usize) {
    let _ = bindings::ts_resource_ex(0, std::ptr::null_mut());
    // Each .so has its own `_tsrm_ls_cache`: update the bridge library's so
    // SG()/CG()/EG() resolve from liboxphp_bridge.so on this thread.
    bindings::oxphp_bridge_tsrm_update();
    bindings::oxphp_bridge_init_ctx();
    bindings::oxphp_bridge_set_worker_id(worker_id as i32);
}

/// A thread of one of the server's PHP pools, as plugins see it: they see the
/// thread start when this is entered and stop when it is dropped — on every
/// way out of the scope that holds it, a panic included. It is entered at the
/// top of the thread's body and dropped before the thread's TSRM resources
/// are released.
pub(crate) struct PhpThread(());

impl PhpThread {
    /// [`attach`] the calling thread, then tell plugins it has started.
    ///
    /// # Safety
    /// As for [`attach`].
    pub(crate) unsafe fn enter(worker_id: usize) -> Self {
        attach(worker_id);
        installed().thread_started();
        PhpThread(())
    }
}

impl Drop for PhpThread {
    fn drop(&mut self) {
        installed().thread_stopping();
    }
}

/// Tell plugins a PHP request is opening, start it on this thread, and tell
/// them it has opened, before its script starts. `false` when
/// `php_request_startup` failed: plugins then saw that request opening, and
/// neither opened nor closed.
///
/// # Safety
/// The thread is attached and has no request open.
pub(crate) unsafe fn open_request(scope: RequestScope) -> bool {
    // Before the startup: it can run userland code (a session that
    // `session.auto_start` decodes wakes its objects up).
    installed().request_frame_opening(scope);
    if bindings::php_request_startup() != 0 {
        return false;
    }
    // The request is open from here: a plugin that panics now would unwind
    // past the shutdown the caller still owes it.
    let request_open = crate::executor::AbortOnUnwind("with a PHP request open");
    installed().request_frame_opened(scope);
    drop(request_open);
    true
}

/// Tell plugins the PHP request open on this thread is about to close, shut
/// it down, then tell plugins it has closed — after `php_request_shutdown`
/// has reset the request heap, and before any other request can start on
/// the thread.
///
/// # Safety
/// A request opened by [`open_request`] is open on this thread.
pub(crate) unsafe fn close_request() {
    // Still open: a plugin that panics here would unwind past the shutdown.
    let request_open = crate::executor::AbortOnUnwind("with a PHP request open");
    installed().request_frame_closing();
    drop(request_open);
    bindings::php_request_shutdown(std::ptr::null_mut());
    installed().request_frame_closed();
}
