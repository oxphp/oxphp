//! End-to-end integration test for trigger → mode propagation.
//! Exercises `ProfilerPlugin`'s request handler through the real
//! plugin dispatcher and asserts the final `RequestReceived.profiling_mode`.

#![cfg(feature = "plugin-profiler")]

use http::{HeaderValue, Method, Request};
use oxphp::events::{EventDispatcher, RequestReceived};
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
        profiling_run_id: None,
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
        profiling_run_id: None,
    }
}

fn init_profiler_dispatcher() -> (PluginManager, EventDispatcher) {
    let mut pm = PluginManager::new();
    pm.add(Box::new(ProfilerPlugin::new()));
    let mut dispatcher = EventDispatcher::new();
    pm.init_all(&mut dispatcher).unwrap();
    (pm, dispatcher)
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
    assert!(
        event.profiling_run_id.is_some(),
        "Triggered request should have a generated run_id"
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
    assert!(event.profiling_run_id.is_none());

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
