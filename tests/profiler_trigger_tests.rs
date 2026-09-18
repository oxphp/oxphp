//! End-to-end integration test for trigger → mode propagation.
//! Exercises `ProfilerPlugin`'s request handler through the real
//! plugin dispatcher and asserts what the event carries away from it: the
//! `profiling_mode` the worker reads, and the metadata entry the profiler's
//! own complete handler reads back after the response is sent.

#![cfg(feature = "plugin-profiler")]

use http::{HeaderValue, Method, Request};
use oxphp::events::{EventDispatcher, EventHandler, Priority, Propagation, RequestReceived};
use oxphp::plugin::PluginManager;
use oxphp::plugins::ox_profiler::ProfilerPlugin;
use oxphp::profiling::ProfilingMode;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Mutex, MutexGuard, OnceLock};

// Tests in this file mutate process-wide env vars (PROFILER_*) and must not
// run in parallel with each other. Acquire this mutex for the whole duration
// of every test that touches env vars.
fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn build_event_with_header(key: &'static str, value: &'static str) -> RequestReceived {
    let (parts, _) = Request::builder()
        .method(Method::GET)
        .uri("/api/ping")
        .header(key, HeaderValue::from_static(value))
        .body(())
        .unwrap()
        .into_parts();
    RequestReceived {
        parts,
        remote_addr: SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 0),
        request_id: "req-test".into(),
        early_response: None,
        metadata: Vec::new(),
        profiling_mode: None,
    }
}

fn plain_event() -> RequestReceived {
    let (parts, _) = Request::builder()
        .method(Method::GET)
        .uri("/api/ping")
        .body(())
        .unwrap()
        .into_parts();
    RequestReceived {
        parts,
        remote_addr: SocketAddr::new(Ipv4Addr::new(127, 0, 0, 1).into(), 0),
        request_id: "req-test".into(),
        early_response: None,
        metadata: Vec::new(),
        profiling_mode: None,
    }
}

/// Read one entry off the event's metadata — the channel the profiler uses to
/// carry its decision to the complete handler.
fn metadata<'a>(event: &'a RequestReceived, key: &str) -> Option<&'a str> {
    event
        .metadata
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

fn init_profiler_dispatcher() -> (PluginManager, EventDispatcher) {
    let mut pm = PluginManager::new();
    pm.add(Box::new(ProfilerPlugin::new()));
    let mut dispatcher = EventDispatcher::new();
    pm.init_all(&mut dispatcher).unwrap();
    (pm, dispatcher)
}

/// Stands in for `ox_otel`'s request handler, which under `OTEL_ENABLED=true`
/// replaces the request id with one derived from the trace context. Only the
/// priority and the one line that matters are copied: this is here to put a
/// handler that rewrites `request_id` *after* the profiler's, not to reproduce
/// how the replacement id is built.
struct RequestIdReplacer;

impl EventHandler<RequestReceived> for RequestIdReplacer {
    fn handle(&self, event: &mut RequestReceived) -> Propagation {
        // Record whether the profiler had already decided by the time this
        // ran. Without it the test cannot tell the two orders apart: the id
        // ends up replaced either way, and only this says the replacement
        // happened *after* the profiler saw the request.
        let profiler_ran = event.metadata.iter().any(|(k, _)| k == "profiler.source");
        event
            .metadata
            .push(("test.profiler_ran_first".into(), profiler_ran.to_string()));
        event.request_id = "4bf92f3577b34da6-00f067aa".into();
        Propagation::Continue
    }

    fn priority(&self) -> Priority {
        -80
    }
}

#[test]
fn triggered_request_propagates_profile_all_mode() {
    let _guard = env_lock();
    std::env::set_var("PROFILER_ENABLED", "true");
    std::env::remove_var("PROFILER_AUTH_TOKEN");

    let (_pm, dispatcher) = init_profiler_dispatcher();
    let mut event = build_event_with_header("x-oxphp-profile", "any-value");
    dispatcher.dispatch(&mut event);

    assert_eq!(
        event.profiling_mode,
        Some(ProfilingMode::ProfileAll),
        "Triggered request should activate ProfileAll mode"
    );
    let source = metadata(&event, "profiler.source");
    assert_eq!(
        source,
        Some("header"),
        "Triggered request should record which trigger matched"
    );
    std::env::remove_var("PROFILER_ENABLED");
}

#[test]
fn untriggered_request_does_not_override_mode() {
    let _guard = env_lock();
    std::env::set_var("PROFILER_ENABLED", "true");
    std::env::remove_var("PROFILER_AUTH_TOKEN");

    let (_pm, dispatcher) = init_profiler_dispatcher();
    let mut event = plain_event();
    dispatcher.dispatch(&mut event);

    assert!(
        event.profiling_mode.is_none(),
        "Untriggered request should leave mode unset (falls through to default)"
    );
    assert_eq!(metadata(&event, "profiler.source"), None);

    std::env::remove_var("PROFILER_ENABLED");
}

#[test]
fn disabled_profiler_never_activates() {
    let _guard = env_lock();
    std::env::set_var("PROFILER_ENABLED", "false");

    let (_pm, dispatcher) = init_profiler_dispatcher();
    let mut event = build_event_with_header("x-oxphp-profile", "x");
    dispatcher.dispatch(&mut event);

    assert!(
        event.profiling_mode.is_none(),
        "Disabled profiler should never activate, even with a header"
    );

    std::env::remove_var("PROFILER_ENABLED");
}

#[test]
fn a_later_handler_still_gets_to_change_the_id_the_run_will_be_filed_under() {
    // The reason nothing is minted at `RequestReceived`: the profiler's
    // handler is not the last one to touch `request_id`. Under
    // `OTEL_ENABLED=true`, `ox_otel` runs after it and replaces the id
    // outright, and the stored run is filed under the replacement — so an id
    // minted here would embed eight characters of a string that survives
    // nowhere else. Registered before the plugin and dispatched through a
    // frozen dispatcher, so the order under test comes from the priorities
    // and not from the order of these two lines.
    let _guard = env_lock();
    std::env::set_var("PROFILER_ENABLED", "true");
    std::env::remove_var("PROFILER_AUTH_TOKEN");

    let mut pm = PluginManager::new();
    pm.add(Box::new(ProfilerPlugin::new()));
    let mut dispatcher = EventDispatcher::new();
    dispatcher.on(RequestIdReplacer);
    pm.init_all(&mut dispatcher).unwrap();
    dispatcher.freeze();

    let mut event = build_event_with_header("x-oxphp-profile", "any-value");
    dispatcher.dispatch(&mut event);

    assert_eq!(
        metadata(&event, "profiler.source"),
        Some("header"),
        "the profiler's handler should have run"
    );
    assert_eq!(
        metadata(&event, "test.profiler_ran_first"),
        Some("true"),
        "the profiler should decide before a handler that rewrites the id"
    );
    assert_eq!(
        event.request_id, "4bf92f3577b34da6-00f067aa",
        "and that handler should still own the final request id"
    );

    std::env::remove_var("PROFILER_ENABLED");
}
