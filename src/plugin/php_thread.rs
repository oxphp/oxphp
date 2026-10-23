//! Lifecycle events of the threads that run PHP, for plugins that keep state
//! per PHP thread, per PHP request or per request fiber.
//!
//! A plugin registers a [`PluginPhpThreadHandler`] with
//! [`PluginContext::on_php_thread`](super::PluginContext::on_php_thread)
//! during `init()`. The handlers are frozen before the first PHP thread
//! starts, and the code that drives a PHP thread raises the events on that
//! thread — every handler runs on the PHP thread the event is about.

use std::any::Any;
use std::cell::RefCell;
use std::sync::OnceLock;

/// How long the PHP request on a thread lasts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestScope {
    /// The request lasts as long as the thread (worker mode, the async pool).
    Thread,
    /// The thread runs one request, then the process shuts down
    /// (`oxphp run`).
    OneRequest,
    /// The thread runs one request after another (traditional, Framework and
    /// SPA routing).
    EachRequest,
}

/// Receives the lifecycle events of PHP threads. Every method defaults to
/// doing nothing, so a handler implements only the events it needs.
///
/// Handlers receive the events that put state on the thread —
/// `thread_started`, `request_frame_opening`, `request_frame_opened` and
/// `fiber_resumed` — in plugin init order, and those that take it off —
/// `thread_stopping`, `request_frame_closing`,
/// `request_frame_closed`, `fiber_parked` and `fiber_dropped` — in the reverse
/// order, so a plugin that depends on another has its own state put on after,
/// and taken off before, the state it depends on.
///
/// Every PHP request the server opens raises the request events; the one
/// OPcache opens inside module startup to run `opcache.preload` raises none.
/// The threads of the server's PHP pools — the HTTP workers and the
/// `oxphp_async()` pool — raise [`thread_started`](Self::thread_started) and
/// [`thread_stopping`](Self::thread_stopping) as well; the main thread of
/// `oxphp run`, which runs its script, does not. The fiber events come from
/// the threads that multiplex request fibers (worker mode); an
/// `oxphp_async()` pool thread raises only `fiber_dropped`, for each task
/// fiber still unfinished when its request shuts down. That pool's threads
/// also raise [`value_leaving_thread`](Self::value_leaving_thread) as they
/// hand a task's result back. [`thread_idle`](Self::thread_idle) comes only
/// from the threads that have a [`PhpThreadWaker`]: the HTTP workers in
/// worker mode.
///
/// A handler must not panic: one that panics while a PHP request is open on
/// the thread ends the process.
pub trait PluginPhpThreadHandler: Send + Sync {
    /// The thread has been attached to PHP and has not opened a request yet.
    fn thread_started(&self) {}

    /// The thread has closed its last request and is about to be detached
    /// from PHP. Raised on every way out of the thread, a panic between
    /// requests included.
    fn thread_stopping(&self) {}

    /// A PHP request is about to start on this thread: `php_request_startup`
    /// has not run yet. The startup can run userland code — with
    /// `session.auto_start` on, decoding the session calls `__wakeup` and
    /// `__unserialize` on its objects, whose classes `opcache.preload` can
    /// define — so state that code has to find goes on here, not in
    /// [`request_frame_opened`](Self::request_frame_opened). When the startup
    /// fails, neither `request_frame_opened` nor `request_frame_closed` is
    /// raised for that request.
    fn request_frame_opening(&self, scope: RequestScope) {
        let _ = scope;
    }

    /// A PHP request has started on this thread (`php_request_startup`
    /// succeeded) and its script has not started yet.
    fn request_frame_opened(&self, scope: RequestScope) {
        let _ = scope;
    }

    /// The PHP request on this thread is about to be shut down:
    /// `php_request_shutdown` has not run yet, and what the request
    /// allocated is still there. The shutdown runs userland code — shutdown
    /// functions, destructors, session handlers — after this event.
    fn request_frame_closing(&self) {}

    /// The PHP request on this thread has been shut down
    /// (`php_request_shutdown` has returned): what the request allocated is
    /// gone, and no other request has started yet.
    fn request_frame_closed(&self) {}

    /// The thread has no request in flight and is waiting for the next one,
    /// and its [`PhpThreadWaker`] has been woken since it last raised this
    /// event. Raised in plugin init order.
    ///
    /// The thread's own PHP request is open — in worker mode it lasts as long
    /// as the thread — so a handler can call into PHP here, but no HTTP
    /// request is in flight for what it runs to belong to: a fatal error in
    /// it retires the worker instead of ending a request, and the pool
    /// replaces the worker. `max_execution_time` does not apply.
    ///
    /// Not raised once the server has begun its graceful drain: a worker
    /// retired then would be replaced in the middle of the shutdown. A wake
    /// that arrives during the drain is taken without raising the event.
    fn thread_idle(&self) {}

    /// The request fiber `fiber_id` has suspended, and its request is being
    /// taken off the thread. State that belongs to that request has to come
    /// off the thread here, or the next request the thread runs while this one
    /// waits finds it.
    fn fiber_parked(&self, fiber_id: u64) {
        let _ = fiber_id;
    }

    /// The request fiber `fiber_id`, parked earlier, is about to resume, and
    /// its request is back on the thread.
    fn fiber_resumed(&self, fiber_id: u64) {
        let _ = fiber_id;
    }

    /// The fiber `fiber_id` has finished or been destroyed, parked or not:
    /// whatever was kept for it can go.
    fn fiber_dropped(&self, fiber_id: u64) {
        let _ = fiber_id;
    }

    /// `bytes`, a value serialized on this thread, is about to be handed to
    /// another thread, and this thread is about to release the value it was
    /// serialized from. A handler whose objects the bytes refer to by id
    /// returns what keeps them alive until the receiving thread has read
    /// the bytes.
    fn value_leaving_thread(&self, bytes: &[u8]) -> Option<Box<dyn Any + Send>> {
        let _ = bytes;
        None
    }
}

/// The registered handlers, in plugin init order.
#[derive(Default)]
pub struct PhpThreadHandlers(Vec<Box<dyn PluginPhpThreadHandler>>);

impl PhpThreadHandlers {
    pub(crate) fn new(handlers: Vec<Box<dyn PluginPhpThreadHandler>>) -> Self {
        Self(handlers)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn thread_started(&self) {
        for h in &self.0 {
            h.thread_started();
        }
    }

    pub fn thread_stopping(&self) {
        for h in self.0.iter().rev() {
            h.thread_stopping();
        }
    }

    pub fn request_frame_opening(&self, scope: RequestScope) {
        for h in &self.0 {
            h.request_frame_opening(scope);
        }
    }

    pub fn request_frame_opened(&self, scope: RequestScope) {
        for h in &self.0 {
            h.request_frame_opened(scope);
        }
    }

    pub fn request_frame_closing(&self) {
        for h in self.0.iter().rev() {
            h.request_frame_closing();
        }
    }

    pub fn request_frame_closed(&self) {
        for h in self.0.iter().rev() {
            h.request_frame_closed();
        }
    }

    pub fn thread_idle(&self) {
        for h in &self.0 {
            h.thread_idle();
        }
    }

    pub fn fiber_parked(&self, fiber_id: u64) {
        for h in self.0.iter().rev() {
            h.fiber_parked(fiber_id);
        }
    }

    pub fn fiber_resumed(&self, fiber_id: u64) {
        for h in &self.0 {
            h.fiber_resumed(fiber_id);
        }
    }

    pub fn fiber_dropped(&self, fiber_id: u64) {
        for h in self.0.iter().rev() {
            h.fiber_dropped(fiber_id);
        }
    }

    /// What every handler needs kept alive for `bytes`, as one value: `None`
    /// when none does, the handler's own value when one does.
    pub fn value_leaving_thread(&self, bytes: &[u8]) -> Option<Box<dyn Any + Send>> {
        let mut kept: Vec<Box<dyn Any + Send>> = self
            .0
            .iter()
            .filter_map(|h| h.value_leaving_thread(bytes))
            .collect();
        match kept.len() {
            0 => None,
            1 => kept.pop(),
            _ => Some(Box::new(kept)),
        }
    }
}

/// Wakes a PHP thread that is waiting for work, so that it raises
/// [`PluginPhpThreadHandler::thread_idle`] without being handed a request.
///
/// A handler takes the waker of its own thread in
/// [`thread_started`](PluginPhpThreadHandler::thread_started) and can wake the
/// thread from any thread afterwards.
#[derive(Clone, Debug)]
pub struct PhpThreadWaker(crossbeam_channel::Sender<()>);

impl PhpThreadWaker {
    /// The waker of the PHP thread this is called on, or `None` on a thread
    /// that never raises `thread_idle`.
    pub fn current() -> Option<Self> {
        WAKER.with(|w| w.borrow().clone())
    }

    /// Have the thread raise `thread_idle` the next time it waits for work —
    /// at once if it is waiting now. Wakes that come before it gets there
    /// raise the event once. Does nothing once the thread has stopped; once
    /// the server has begun its graceful drain, the thread wakes but raises
    /// nothing.
    pub fn wake(&self) {
        let _ = self.0.try_send(());
    }
}

thread_local! {
    static WAKER: RefCell<Option<PhpThreadWaker>> = const { RefCell::new(None) };
}

/// Give the calling thread a [`PhpThreadWaker`] and return the end its wait
/// for work reads. Called by the code that drives a PHP thread able to raise
/// `thread_idle`, before the thread raises `thread_started`.
#[cfg_attr(not(feature = "php"), allow(dead_code))]
pub(crate) fn install_waker() -> crossbeam_channel::Receiver<()> {
    let (tx, rx) = crossbeam_channel::bounded(1);
    WAKER.with(|w| *w.borrow_mut() = Some(PhpThreadWaker(tx)));
    rx
}

static INSTALLED: OnceLock<PhpThreadHandlers> = OnceLock::new();
static NONE: PhpThreadHandlers = PhpThreadHandlers(Vec::new());

/// Freeze the handlers every PHP thread raises its events on. Called once,
/// after plugin init and before the first PHP thread starts.
#[cfg_attr(not(feature = "php"), allow(dead_code))]
pub(crate) fn install(handlers: PhpThreadHandlers) {
    if INSTALLED.set(handlers).is_err() {
        tracing::warn!("PHP thread handlers installed twice; keeping the first set");
    }
}

/// The handlers frozen by [`install`], or none before it has run.
#[inline]
pub(crate) fn installed() -> &'static PhpThreadHandlers {
    INSTALLED.get().unwrap_or(&NONE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Records every event it receives, tagged with its own name.
    struct Recorder {
        name: &'static str,
        log: Arc<Mutex<Vec<String>>>,
        keep: bool,
    }

    impl Recorder {
        fn push(&self, event: &str) {
            self.log
                .lock()
                .unwrap()
                .push(format!("{}:{}", self.name, event));
        }
    }

    impl PluginPhpThreadHandler for Recorder {
        fn thread_started(&self) {
            self.push("started");
        }
        fn thread_stopping(&self) {
            self.push("stopping");
        }
        fn request_frame_opening(&self, scope: RequestScope) {
            self.push(&format!("opening({scope:?})"));
        }
        fn request_frame_opened(&self, scope: RequestScope) {
            self.push(&format!("opened({scope:?})"));
        }
        fn request_frame_closing(&self) {
            self.push("closing");
        }
        fn request_frame_closed(&self) {
            self.push("closed");
        }
        fn thread_idle(&self) {
            self.push("idle");
        }
        fn fiber_parked(&self, fiber_id: u64) {
            self.push(&format!("parked({fiber_id})"));
        }
        fn fiber_resumed(&self, fiber_id: u64) {
            self.push(&format!("resumed({fiber_id})"));
        }
        fn fiber_dropped(&self, fiber_id: u64) {
            self.push(&format!("dropped({fiber_id})"));
        }
        fn value_leaving_thread(&self, bytes: &[u8]) -> Option<Box<dyn Any + Send>> {
            self.push(&format!("leaving({})", bytes.len()));
            self.keep
                .then(|| Box::new(self.name) as Box<dyn Any + Send>)
        }
    }

    fn two_recorders(keep_a: bool, keep_b: bool) -> (PhpThreadHandlers, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let handlers = PhpThreadHandlers::new(vec![
            Box::new(Recorder {
                name: "a",
                log: Arc::clone(&log),
                keep: keep_a,
            }),
            Box::new(Recorder {
                name: "b",
                log: Arc::clone(&log),
                keep: keep_b,
            }),
        ]);
        (handlers, log)
    }

    #[test]
    fn events_that_put_state_on_run_in_init_order_and_the_rest_in_reverse() {
        let (h, log) = two_recorders(false, false);
        h.thread_started();
        h.request_frame_opening(RequestScope::EachRequest);
        h.request_frame_opened(RequestScope::EachRequest);
        h.thread_idle();
        h.fiber_parked(7);
        h.fiber_resumed(7);
        h.fiber_dropped(7);
        h.request_frame_closing();
        h.request_frame_closed();
        h.thread_stopping();
        assert_eq!(
            *log.lock().unwrap(),
            vec![
                "a:started",
                "b:started",
                "a:opening(EachRequest)",
                "b:opening(EachRequest)",
                "a:opened(EachRequest)",
                "b:opened(EachRequest)",
                "a:idle",
                "b:idle",
                "b:parked(7)",
                "a:parked(7)",
                "a:resumed(7)",
                "b:resumed(7)",
                "b:dropped(7)",
                "a:dropped(7)",
                "b:closing",
                "a:closing",
                "b:closed",
                "a:closed",
                "b:stopping",
                "a:stopping",
            ]
        );
    }

    #[test]
    fn a_handler_that_overrides_nothing_receives_every_event_silently() {
        struct Quiet;
        impl PluginPhpThreadHandler for Quiet {}
        let h = PhpThreadHandlers::new(vec![Box::new(Quiet)]);
        h.thread_started();
        h.request_frame_opening(RequestScope::Thread);
        h.request_frame_opened(RequestScope::Thread);
        h.thread_idle();
        h.fiber_parked(1);
        h.fiber_resumed(1);
        h.fiber_dropped(1);
        h.request_frame_closing();
        h.request_frame_closed();
        h.thread_stopping();
        assert!(h.value_leaving_thread(b"x").is_none());
    }

    #[test]
    fn a_thread_without_an_installed_waker_has_none() {
        let none = std::thread::spawn(|| PhpThreadWaker::current().is_none())
            .join()
            .unwrap();
        assert!(none);
    }

    /// Wakes that come while the thread is busy are one wake once it waits,
    /// and a waker outliving its thread is harmless.
    #[test]
    fn wakes_before_the_wait_collapse_into_one() {
        let (rx, waker) = std::thread::spawn(|| {
            let rx = install_waker();
            (rx, PhpThreadWaker::current().expect("installed"))
        })
        .join()
        .unwrap();
        waker.wake();
        waker.clone().wake();
        assert!(rx.try_recv().is_ok());
        assert!(
            rx.try_recv().is_err(),
            "the second wake was folded into the first"
        );
        drop(rx);
        waker.wake();
    }

    #[test]
    fn no_handlers_keep_nothing_alive() {
        let h = PhpThreadHandlers::default();
        assert!(h.is_empty());
        assert!(h.value_leaving_thread(b"abc").is_none());
    }

    #[test]
    fn a_value_kept_by_no_handler_is_none_but_every_handler_was_asked() {
        let (h, log) = two_recorders(false, false);
        assert!(h.value_leaving_thread(b"abc").is_none());
        assert_eq!(*log.lock().unwrap(), vec!["a:leaving(3)", "b:leaving(3)"]);
    }

    #[test]
    fn a_value_kept_by_one_handler_is_that_handlers_own() {
        let (h, _log) = two_recorders(false, true);
        let kept = h.value_leaving_thread(b"abc").expect("b keeps it");
        assert_eq!(kept.downcast_ref::<&str>(), Some(&"b"));
    }

    #[test]
    fn a_value_kept_by_several_handlers_holds_all_of_theirs() {
        let (h, _log) = two_recorders(true, true);
        let kept = h.value_leaving_thread(b"abc").expect("both keep it");
        let all = kept
            .downcast_ref::<Vec<Box<dyn Any + Send>>>()
            .expect("several values travel as one");
        let names: Vec<&str> = all
            .iter()
            .map(|v| *v.downcast_ref::<&str>().unwrap())
            .collect();
        assert_eq!(names, vec!["a", "b"]);
    }
}
