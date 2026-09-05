use std::convert::Infallible;
use std::net::IpAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use bytes::Bytes;
use http::{Request, Response, StatusCode};
use hyper::body::Incoming;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use hyper_util::server::conn::auto::Builder;
use tokio::net::TcpListener;

use crate::config::{Config, IpAllowList};
use crate::executor::ScriptExecutor;
use crate::metrics::Metrics;
use crate::plugin::handler::PluginInternalRequest;
use crate::plugin::PluginManager;
use crate::types::{full_body, ResponseBody};

/// Health-probe paths that stay reachable regardless of `INTERNAL_ALLOW_IPS`:
/// the probe source is the orchestrator/node, not the metrics scraper, so
/// gating them would make a pod kill itself.
const PROBE_PATHS: &[&str] = &[
    "/health",
    "/healthz",
    "/health/liveness",
    "/readyz",
    "/health/readiness",
    "/startupz",
    "/health/startup",
];

/// Decide whether `peer_ip` may reach `path`. Probe paths are always allowed.
/// Every other path is gated by `allow` when set; an unset/empty allow-list
/// permits all peers (the prior behavior). Loopback is not special-cased — to
/// keep localhost access, list `127.0.0.1/32` in `INTERNAL_ALLOW_IPS`.
fn gate_allows(path: &str, peer_ip: IpAddr, allow: Option<&IpAllowList>) -> bool {
    if PROBE_PATHS.contains(&path) {
        return true;
    }
    match allow {
        Some(list) => list.contains(peer_ip),
        None => true,
    }
}

fn forbidden_response() -> Response<ResponseBody> {
    Response::builder()
        .status(StatusCode::FORBIDDEN)
        .body(full_body(Bytes::from_static(b"403 Forbidden")))
        .unwrap()
}

/// Run the internal HTTP server for health, metrics, and config endpoints.
/// This server listens on a separate port and is only started when
/// `INTERNAL_ADDR` is set. The listener is bound in `main()` before any
/// privilege drop and handed here as a non-blocking std socket.
pub async fn run_internal_server(
    listener: std::net::TcpListener,
    metrics: Arc<Metrics>,
    config: Arc<Config>,
    executor: Arc<dyn ScriptExecutor>,
    plugin_manager: Arc<PluginManager>,
    shutdown: Arc<AtomicBool>,
) -> Result<(), crate::types::BoxError> {
    let listener = TcpListener::from_std(listener)?;
    let local_addr = listener.local_addr()?;
    tracing::info!(addr = %local_addr, "Internal server listening");

    loop {
        let (stream, remote) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!(error = %e, "Internal server accept error");
                continue;
            }
        };
        let peer_ip = remote.ip();

        let metrics = Arc::clone(&metrics);
        let config = Arc::clone(&config);
        let executor = Arc::clone(&executor);
        let pm = Arc::clone(&plugin_manager);
        let shutdown = Arc::clone(&shutdown);

        tokio::spawn(async move {
            let service = service_fn(move |req| {
                let metrics = Arc::clone(&metrics);
                let config = Arc::clone(&config);
                let executor = Arc::clone(&executor);
                let pm = Arc::clone(&pm);
                let shutdown = Arc::clone(&shutdown);
                async move {
                    handle_internal_request(
                        req, peer_ip, &metrics, &config, &*executor, &pm, &shutdown,
                    )
                }
            });

            let io = TokioIo::new(stream);
            let builder = Builder::new(hyper_util::rt::TokioExecutor::new());
            if let Err(e) = builder.serve_connection(io, service).await {
                tracing::debug!(error = %e, "Internal server connection error");
            }
        });
    }
}

fn handle_internal_request(
    req: Request<Incoming>,
    peer_ip: IpAddr,
    metrics: &Metrics,
    config: &Config,
    executor: &dyn ScriptExecutor,
    plugin_manager: &PluginManager,
    shutdown: &AtomicBool,
) -> Result<Response<ResponseBody>, Infallible> {
    if !gate_allows(
        req.uri().path(),
        peer_ip,
        config.internal_allow_ips.as_ref(),
    ) {
        return Ok(forbidden_response());
    }
    let response = match req.uri().path() {
        "/health/liveness" | "/healthz" => liveness_response(),
        "/health/readiness" | "/readyz" => {
            readiness_response(metrics, executor, plugin_manager, shutdown)
        }
        "/health/startup" | "/startupz" => startup_response(executor),
        "/health" => health_response(metrics, executor, plugin_manager),
        "/metrics" => metrics_response(metrics, plugin_manager),
        "/config" => config_response(config, plugin_manager),
        p if p.starts_with("/__") => {
            let internal_req = PluginInternalRequest {
                method: req.method(),
                path: p,
                headers: req.headers(),
                query: req.uri().query(),
            };
            plugin_manager
                .handle_internal_route(&internal_req)
                .unwrap_or_else(|| {
                    Response::builder()
                        .status(StatusCode::NOT_FOUND)
                        .body(full_body(Bytes::from_static(b"404 Not Found")))
                        .unwrap()
                })
        }
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(full_body(Bytes::from_static(b"404 Not Found")))
            .unwrap(),
    };
    Ok(response)
}

fn liveness_response() -> Response<ResponseBody> {
    Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "text/plain")
        .body(full_body(Bytes::from_static(b"liveness")))
        .unwrap()
}

fn readiness_response(
    metrics: &Metrics,
    executor: &dyn ScriptExecutor,
    plugin_manager: &PluginManager,
    shutdown: &AtomicBool,
) -> Response<ResponseBody> {
    let is_ready = !shutdown.load(Ordering::SeqCst)
        && executor.is_healthy()
        && !metrics.pool_stalled()
        && !plugin_manager
            .health_all()
            .iter()
            .any(|(_, h)| *h == crate::plugin::PluginHealth::Failed);

    let status = if is_ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain")
        .body(full_body(Bytes::from_static(b"readiness")))
        .unwrap()
}

fn startup_response(executor: &dyn ScriptExecutor) -> Response<ResponseBody> {
    let status = if executor.is_healthy() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    Response::builder()
        .status(status)
        .header(http::header::CONTENT_TYPE, "text/plain")
        .body(full_body(Bytes::from_static(b"startup")))
        .unwrap()
}

/// Build the `/health` body and the status that goes with it.
///
/// Split out so the two can be asserted together: the JSON says which
/// subsystem is at fault and the status is what a dashboard alerts on, and a
/// body that named a fault under a `200` would be read as a healthy server by
/// everything that does not parse it.
fn build_health_json(
    metrics: &Metrics,
    executor: &dyn ScriptExecutor,
    plugin_manager: &PluginManager,
) -> (StatusCode, serde_json::Value) {
    let executor_healthy = executor.is_healthy();

    // Check plugin health
    let plugin_health = plugin_manager.health_all();
    let any_failed = plugin_health
        .iter()
        .any(|(_, h)| *h == crate::plugin::PluginHealth::Failed);

    // A wedged pool leaves every worker thread alive, so `executor_healthy`
    // stays true through it and carries none of this; it is a separate fact
    // about the same pool and gets a field of its own.
    let pool_stalled = metrics.pool_stalled();

    let degraded = !executor_healthy || any_failed || pool_stalled;
    let status_str = if degraded { "degraded" } else { "ok" };
    let http_status = if degraded {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };

    // Build plugins health JSON
    let plugins_json: serde_json::Value = plugin_health
        .iter()
        .map(|(name, health)| (name.to_string(), serde_json::json!(health.as_str())))
        .collect::<serde_json::Map<String, serde_json::Value>>()
        .into();

    let body = serde_json::json!({
        "status": status_str,
        "uptime_secs": metrics.uptime().as_secs(),
        "total_requests": metrics.total_requests(),
        "active_connections": metrics.active_connections(),
        "executor_healthy": executor_healthy,
        "pool_stalled": pool_stalled,
        "plugins": plugins_json,
    });

    (http_status, body)
}

fn health_response(
    metrics: &Metrics,
    executor: &dyn ScriptExecutor,
    plugin_manager: &PluginManager,
) -> Response<ResponseBody> {
    let (http_status, body) = build_health_json(metrics, executor, plugin_manager);

    Response::builder()
        .status(http_status)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(full_body(Bytes::from(body.to_string())))
        .unwrap()
}

fn metrics_response(metrics: &Metrics, plugin_manager: &PluginManager) -> Response<ResponseBody> {
    let mut body = metrics.to_prometheus();
    plugin_manager.collect_metrics(&mut body);

    Response::builder()
        .status(StatusCode::OK)
        .header(
            http::header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )
        .body(full_body(Bytes::from(body)))
        .unwrap()
}

/// Build the `/config` JSON, merging the plugin config blob and scrubbing
/// topology/path details (`internal_addr`, `error_pages_dir`) that aid an
/// attacker and are not needed by metrics scrapers.
fn build_config_json(config: &Config, plugin_manager: &PluginManager) -> serde_json::Value {
    let mut body = config.to_json();
    if let Some(obj) = body.as_object_mut() {
        obj.insert("plugins".to_string(), plugin_manager.config_json());
        obj.remove("internal_addr");
        obj.remove("error_pages_dir");
    }
    body
}

fn config_response(config: &Config, plugin_manager: &PluginManager) -> Response<ResponseBody> {
    let body = build_config_json(config, plugin_manager);
    Response::builder()
        .status(StatusCode::OK)
        .header(http::header::CONTENT_TYPE, "application/json")
        .body(full_body(Bytes::from(body.to_string())))
        .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IpAllowList;
    use crate::events::EventDispatcher;
    use crate::executor::stub::StubExecutor;
    use crate::executor::ScriptExecutor;
    use crate::plugin::{Plugin, PluginContext, PluginError, PluginHealth, PluginManager};
    use crate::types::ScriptRequest;
    use std::net::IpAddr;
    use std::sync::atomic::AtomicBool;

    #[test]
    fn test_gate_probe_paths_always_allowed() {
        let allow = IpAllowList::from_spec("10.0.0.0/8");
        let outsider: IpAddr = "203.0.113.5".parse().unwrap();
        for p in [
            "/health",
            "/healthz",
            "/health/liveness",
            "/readyz",
            "/health/readiness",
            "/startupz",
            "/health/startup",
        ] {
            assert!(gate_allows(p, outsider, Some(&allow)), "{p} must stay open");
        }
    }

    #[test]
    fn test_gate_allowed_peer_reaches_gated_path() {
        let allow = IpAllowList::from_spec("10.0.0.0/8");
        let peer: IpAddr = "10.1.2.3".parse().unwrap();
        assert!(gate_allows("/metrics", peer, Some(&allow)));
        assert!(gate_allows("/config", peer, Some(&allow)));
    }

    #[test]
    fn test_gate_non_allowed_peer_blocked_on_gated_path() {
        let allow = IpAllowList::from_spec("10.0.0.0/8");
        let peer: IpAddr = "203.0.113.5".parse().unwrap();
        assert!(!gate_allows("/metrics", peer, Some(&allow)));
        assert!(!gate_allows("/config", peer, Some(&allow)));
        assert!(!gate_allows("/__profiler", peer, Some(&allow)));
    }

    #[test]
    fn test_gate_no_allowlist_allows_all() {
        let peer: IpAddr = "203.0.113.5".parse().unwrap();
        assert!(gate_allows("/metrics", peer, None));
        assert!(gate_allows("/config", peer, None));
    }

    #[test]
    fn test_gate_loopback_not_auto_allowed() {
        let allow = IpAllowList::from_spec("10.0.0.0/8");
        let peer: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(!gate_allows("/metrics", peer, Some(&allow)));
    }

    /// Executor that always reports unhealthy.
    struct UnhealthyExecutor;
    impl ScriptExecutor for UnhealthyExecutor {
        fn execute(&self, _: ScriptRequest) -> crate::executor::ExecuteResult {
            unimplemented!()
        }
        fn shutdown(&self) {}
        fn is_healthy(&self) -> bool {
            false
        }
    }

    /// Plugin that always reports Failed health.
    struct FailedPlugin;
    impl Plugin for FailedPlugin {
        fn name(&self) -> &'static str {
            "failed-test"
        }
        fn version(&self) -> &'static str {
            "0.0.0"
        }
        fn init(&mut self, _ctx: &mut PluginContext) -> Result<(), PluginError> {
            Ok(())
        }
        fn health(&self) -> PluginHealth {
            PluginHealth::Failed
        }
    }

    #[test]
    fn test_config_json_scrubs_sensitive_keys() {
        let mut config = Config::test_minimal();
        config.internal_addr = Some("0.0.0.0:9090".to_string());
        config.error_pages_dir = Some("/etc/oxphp/errors".to_string());
        let pm = PluginManager::new();

        let body = build_config_json(&config, &pm);
        let obj = body.as_object().expect("config json is an object");

        assert!(
            !obj.contains_key("internal_addr"),
            "internal_addr must be scrubbed"
        );
        assert!(
            !obj.contains_key("error_pages_dir"),
            "error_pages_dir must be scrubbed"
        );
        assert!(obj.contains_key("plugins"), "plugins block still present");
        assert!(
            obj.contains_key("listen_addr"),
            "non-sensitive keys still present"
        );
    }

    #[test]
    fn test_liveness_always_200() {
        let resp = liveness_response();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(http::header::CONTENT_TYPE).unwrap(),
            "text/plain"
        );
    }

    #[test]
    fn test_startup_healthy() {
        let executor = StubExecutor::new();
        let resp = startup_response(&executor);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn test_startup_unhealthy() {
        let executor = UnhealthyExecutor;
        let resp = startup_response(&executor);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_readiness_healthy() {
        let executor = StubExecutor::new();
        let pm = PluginManager::new();
        let shutdown = AtomicBool::new(false);
        let resp = readiness_response(&Metrics::new(), &executor, &pm, &shutdown);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn test_readiness_503_on_shutdown() {
        let executor = StubExecutor::new();
        let pm = PluginManager::new();
        let shutdown = AtomicBool::new(true);
        let resp = readiness_response(&Metrics::new(), &executor, &pm, &shutdown);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_readiness_503_on_executor_unhealthy() {
        let executor = UnhealthyExecutor;
        let pm = PluginManager::new();
        let shutdown = AtomicBool::new(false);
        let resp = readiness_response(&Metrics::new(), &executor, &pm, &shutdown);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn test_readiness_503_on_plugin_failed() {
        let executor = StubExecutor::new();
        let mut pm = PluginManager::new();
        pm.add(Box::new(FailedPlugin));
        let mut dispatcher = EventDispatcher::new();
        pm.init_all(&mut dispatcher).unwrap();
        let shutdown = AtomicBool::new(false);
        let resp = readiness_response(&Metrics::new(), &executor, &pm, &shutdown);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    /// Metrics carrying the supervisor's verdict that the pool is wedged.
    fn stalled_metrics() -> Metrics {
        let metrics = Metrics::new();
        metrics.set_pool_stalled(true);
        metrics
    }

    #[test]
    fn readiness_is_503_while_the_pool_is_wedged() {
        // A wedged pool keeps every thread alive and every plugin happy, so
        // the three conditions readiness had all read healthy while nothing
        // the instance was sent could ever be answered. The probe reaches the
        // internal listener, which goes through neither the connection budget
        // nor admission, so it is the one thing still able to say so.
        let executor = StubExecutor::new();
        let pm = PluginManager::new();
        let shutdown = AtomicBool::new(false);
        let resp = readiness_response(&stalled_metrics(), &executor, &pm, &shutdown);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
    }

    #[test]
    fn readiness_is_200_while_the_pool_is_merely_overloaded() {
        // Shedding is not a fault: an instance answering 529 is answering, and
        // taking it out of rotation moves its load onto its neighbours, which
        // under an even overload takes every replica out at once. Nothing but
        // a wedge sets the flag, and this pins that the probe reads that flag
        // rather than any of the load series beside it.
        let metrics = Metrics::new();
        metrics.request_admission_refused(crate::executor::admission::ShedReason::WaitTimeout);
        metrics.request_admission_refused(crate::executor::admission::ShedReason::WaitingFull);
        metrics.accept_stall_begin();
        assert!(!metrics.pool_stalled());

        let executor = StubExecutor::new();
        let pm = PluginManager::new();
        let shutdown = AtomicBool::new(false);
        let resp = readiness_response(&metrics, &executor, &pm, &shutdown);
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[test]
    fn health_reports_a_wedged_pool_as_degraded() {
        // `/health` is what dashboards and container health checks read, and
        // its body is where the reason lives: `executor_healthy` stays true
        // through a wedge — the threads are all there — so without a field of
        // its own the state has no name in this response.
        let executor = StubExecutor::new();
        let pm = PluginManager::new();

        let (status, body) = build_health_json(&Metrics::new(), &executor, &pm);
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["pool_stalled"], serde_json::json!(false));
        assert_eq!(body["status"], serde_json::json!("ok"));

        let (status, body) = build_health_json(&stalled_metrics(), &executor, &pm);
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(body["pool_stalled"], serde_json::json!(true));
        assert_eq!(body["status"], serde_json::json!("degraded"));
        assert_eq!(
            body["executor_healthy"],
            serde_json::json!(true),
            "the threads are alive — the wedge is a separate field, not this one"
        );
    }
}
