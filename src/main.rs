#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

mod logging;
mod privdrop;
mod startup_identity;

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::sync::OnceLock;
use tokio::net::TcpListener;
use tokio::runtime::Handle;
use tokio::signal;
use tokio::sync::Semaphore;

/// Process-global Tokio runtime handle for async promise await operations.
/// Set once in async_main(), read by PHP worker threads for block_on().
pub static TOKIO_HANDLE: OnceLock<Handle> = OnceLock::new();

use oxphp::cli;
use oxphp::config;
use oxphp::events::EventDispatcher;
use oxphp::executor;
use oxphp::handlers;
use oxphp::metrics::Metrics;
use oxphp::plugin::PluginManager;
use oxphp::server;
use oxphp::types;

fn main() -> Result<(), types::BoxError> {
    // Handle CLI flags before any expensive startup (plugin init, PHP MINIT,
    // Tokio runtime). Terminal commands (--help, --version, `config --check`,
    // bad args) exit directly from inside dispatch() with plain-text UX output.
    // The returned role selects what `main` does next: `serve` falls through to
    // the HTTP startup below; `run` executes a single PHP script under CLI
    // semantics and exits with the script's code — entirely separate from the
    // HTTP path (no JSON logging on stdout, no listener).
    let serve_opts = match cli::dispatch() {
        cli::Role::Serve(opts) => opts,
        cli::Role::Run(opts) => {
            // Drop privileges before MINIT / script execution when `--user` was
            // given (k8s Job as non-root). The one-shot path is single-threaded
            // and binds no socket, so the drop is simpler than serve's. Logging
            // is not initialised yet, so the drop's tracing line is silent — the
            // run role uses plain stderr UX, not JSON logs.
            if let Err(e) = privdrop::apply_drop(&opts.user) {
                eprintln!("oxphp: {e}");
                std::process::exit(1);
            }
            std::process::exit(oxphp::frontend::run_cli(opts))
        }
    };

    // JSON logging active from here on — every subsequent startup error is
    // structured. Guard held in main() so the non-blocking writer drains on
    // normal shutdown and the tokio runtime panic path alike.
    let _log_guard = logging::init()?;

    let mut config = Arc::new(match config::Config::from_env() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(error = %e, "config error");
            std::process::exit(1);
        }
    });

    // ── Bind + privilege drop, while still single-threaded ──
    // Bind every listener now, before spawning anything (supervisor, executor
    // workers, Tokio runtime, async pool), so a privileged port (:80/:443) can
    // be bound under the starting user's privileges and the process can then
    // drop to a non-root user. The sockets are non-blocking std sockets that
    // the Tokio runtime re-attaches via `from_std` once it is built. With only
    // the main thread (plus the logging writer) alive, the drop sidesteps the
    // cross-thread `setuid` broadcast. `serve --user` requires starting as
    // root; the drop is irreversible and happens before any request-handling
    // thread exists, so nothing in the request path ever runs as root.
    let http_listener = std::net::TcpListener::bind(&config.server.listen_addr)
        .map_err(|e| format!("failed to bind {}: {e}", config.server.listen_addr))?;
    http_listener.set_nonblocking(true)?;

    let internal_listener = match &config.internal_addr {
        Some(addr) => {
            let listener = std::net::TcpListener::bind(addr)
                .map_err(|e| format!("failed to bind internal address {addr}: {e}"))?;
            listener.set_nonblocking(true)?;
            Some(listener)
        }
        None => None,
    };

    // Warn when the internal server is reachable off-host without an allow-list.
    // The warning is private-aware and suppressed once INTERNAL_ALLOW_IPS is set,
    // so it signals real exposure instead of firing for every deployment.
    if let Some(listener) = &internal_listener {
        if config.internal_allow_ips.is_none() {
            let bound = listener.local_addr()?;
            match config::classify_bind_exposure(bound.ip()) {
                config::BindExposure::Loopback => {}
                config::BindExposure::Private => tracing::info!(
                    addr = %bound,
                    "Internal server bound to a private address; set INTERNAL_ALLOW_IPS to restrict which hosts may reach /metrics and /config"
                ),
                config::BindExposure::Exposed => tracing::warn!(
                    addr = %bound,
                    "Internal server is reachable off-host with no INTERNAL_ALLOW_IPS set — /metrics and /config are exposed; set INTERNAL_ALLOW_IPS or bind a loopback address"
                ),
            }
        }
    }

    privdrop::apply_drop(&serve_opts.drop_to)?;

    // Report effective uid/gid + supplementary groups AFTER any privilege drop,
    // so the log reflects the post-drop identity and the "running as root"
    // warning stays silent once we have dropped.
    startup_identity::log_startup_identity();

    // Create metrics early — needed by executor for worker metrics. Sized
    // by `max_worker_count()` rather than the initial pool size: dynamic-mode
    // scale-ups and traditional-mode respawns hand out IDs up to this limit
    // (see executor::sapi::pool — IDs are recycled within the same range so
    // they never exceed it), and the supervisor's per-slot observe_* helpers
    // need a slot for every possible live worker.
    let metrics = Arc::new(Metrics::new_with_workers(
        config.worker_mode.max_worker_count(),
    ));

    // Initialize plugins BEFORE PHP startup so MINIT can register plugin
    // functions with Zend (OPcache needs them at compile time).
    let mut dispatcher = EventDispatcher::new();
    let mut plugin_manager = PluginManager::new();
    #[cfg(feature = "plugin-otel")]
    plugin_manager.add(Box::new(oxphp::plugins::ox_otel::OtelPlugin::new()));
    #[cfg(feature = "plugin-apm")]
    plugin_manager.add(Box::new(oxphp::plugins::ox_apm::ApmPlugin::new()));
    #[cfg(feature = "plugin-async")]
    plugin_manager.add(Box::new(oxphp::plugins::ox_async::AsyncPlugin::new()));
    #[cfg(feature = "plugin-profiler")]
    plugin_manager.add(Box::new(oxphp::plugins::ox_profiler::ProfilerPlugin::new()));
    #[cfg(feature = "plugin-shared")]
    plugin_manager.add(Box::new(oxphp::plugins::ox_shared::SharedPlugin::default()));
    plugin_manager.init_all(&mut dispatcher)?;

    // Apply core flags set by plugins during init (e.g. OTel enables trace context).
    if !config.trace_context
        && plugin_manager
            .core_flag("trace_context")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false)
    {
        Arc::get_mut(&mut config)
            .expect("config Arc not yet shared")
            .trace_context = true;
    }

    #[cfg(feature = "php")]
    {
        // Register request accessor callbacks for the HTTP Object API. This is
        // an HTTP-frontend concern (not a plugin artifact), so it lives here
        // rather than in the shared pre-MINIT bootstrap below.
        oxphp::php::sapi::register_request_accessors();

        // Register plugin functions / classes / decorators with Zend before
        // MINIT (shared with the `oxphp run` frontend, single source of order).
        let registry = oxphp::php::bootstrap::register_plugin_artifacts(
            &mut plugin_manager,
            config.superglobals_enabled,
        );
        tracing::info!(
            rust_decorators = registry.rust_decorator_count(),
            "Decorator registry initialized"
        );
    }

    // Initialise worker registry before workers are spawned so slots exist
    // when workers register their EG(vm_interrupt) address. Sized by the
    // maximum worker count so dynamic-mode scale-ups and respawned IDs
    // (recycled within `0..max`) all map to a real slot.
    oxphp::php::worker_registry::init_workers(config.worker_mode.max_worker_count());

    // Spawn the per-second observability supervisor (no automatic
    // intervention — operators react to the exposed metrics).
    let supervisor_shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _supervisor_handle = oxphp::php::supervisor::Supervisor::production(Arc::clone(&metrics))
        .spawn(Arc::clone(&supervisor_shutdown));

    // Create executor AFTER plugin functions are on the bridge —
    // php_module_startup() (MINIT) registers them with Zend.
    let executor: Arc<dyn executor::ScriptExecutor> =
        Arc::from(executor::create_executor(&config, Arc::clone(&metrics)));

    // Install crash signal handlers AFTER php_module_startup() to override
    // PHP's zend_signal handlers. Writes diagnostic to stderr + /tmp/oxphp-crash.log.
    install_crash_handlers();

    let async_pool = oxphp::executor::async_pool::AsyncWorkerPool::new(
        config.async_workers,
        config.async_queue_capacity,
        Some(Arc::clone(&metrics)),
    );

    let cpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let tokio_workers: usize = std::env::var("TOKIO_WORKERS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or((cpu / 2).max(1));

    let runtime = if tokio_workers > 1 {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(tokio_workers)
            // Tokio workers only do I/O + channel ops (PHP runs on OS threads),
            // so 512KB stack is sufficient. Reduces memory per worker from 2MB.
            .thread_stack_size(512 * 1024)
            // Lower global_queue_interval improves fairness and tail latency
            // under high concurrency (default is 61).
            .global_queue_interval(32)
            .enable_all()
            .build()?
    } else {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
    };
    let result = runtime.block_on(async_main(
        config,
        Arc::clone(&executor),
        metrics,
        dispatcher,
        plugin_manager,
        async_pool,
        http_listener,
        internal_listener,
    ));

    // Shutdown ordering is load-bearing. The PHP engine teardown
    // (php_module_shutdown/sapi_shutdown/tsrm_shutdown) lives in
    // SapiExecutor::drop and MUST run on the main thread — the same thread
    // that called php_module_startup. Tokio tasks (HTTP serving + internal
    // server) hold Arc<dyn ScriptExecutor> clones; if a task dropped the last
    // reference on a worker thread, sapi_flush() would dereference a NULL SAPI
    // context and crash the process during a graceful stop. Dropping the
    // runtime first releases every task-held clone while this thread still
    // owns one strong reference, so the executor's Drop is guaranteed to fire
    // below, on the main thread.
    drop(runtime);
    drop(executor);

    result
}

#[allow(clippy::too_many_arguments)]
async fn async_main(
    config: Arc<config::Config>,
    executor: Arc<dyn executor::ScriptExecutor>,
    metrics: Arc<Metrics>,
    mut dispatcher: EventDispatcher,
    plugin_manager: PluginManager,
    mut async_pool: Option<executor::async_pool::AsyncWorkerPool>,
    http_listener: std::net::TcpListener,
    internal_listener: Option<std::net::TcpListener>,
) -> Result<(), types::BoxError> {
    TOKIO_HANDLE
        .set(Handle::current())
        .expect("TOKIO_HANDLE already set");

    #[cfg(feature = "php")]
    if let Some(ref mut pool) = async_pool {
        pool.start();
        oxphp::php::sapi::set_global_async_tx(pool.task_sender());
        let inflight_cap = config.async_max_fibers.saturating_mul(pool.worker_count());
        let inflight = Arc::new(oxphp::executor::async_fiber::InFlightCounter::new(
            inflight_cap,
        ));
        metrics.set_async_inflight(Arc::clone(&inflight));
        oxphp::php::sapi::set_global_async_inflight(inflight);
        oxphp::php::sapi::set_async_tokio_handle(Handle::current());
        oxphp::php::sapi::register_async_callbacks();
        oxphp::php::sapi::set_async_metrics(Arc::clone(&metrics));
    }

    // Register fiber scheduler callbacks (try_recv, prepare_request)
    #[cfg(feature = "php")]
    oxphp::php::sapi::register_fiber_callbacks();

    let entry_extension = config
        .entry_file
        .as_ref()
        .and_then(|p| p.extension().and_then(|s| s.to_str()))
        .map(|s| s.to_ascii_lowercase());

    let mode = match (config.worker_mode_enabled, entry_extension.as_deref()) {
        (true, Some("php")) => "worker",
        (false, Some("php")) => "framework",
        (false, Some(_)) => "static-fallback",
        (false, None) => "direct-mapping",
        // (true, _) without a `.php` ENTRY_FILE is rejected by Config::validate
        // before we get here.
        _ => unreachable!("worker mode + non-php entry_file should be rejected by validate"),
    };

    let entry_file_display = config.entry_file.as_ref().map(|p| p.display().to_string());
    tracing::info!(
        event = "mode_decided",
        mode = mode,
        entry = entry_file_display.as_deref(),
        workers = config.worker_mode.worker_count(),
        "OxPHP routing mode decided"
    );

    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        listen_addr = %config.server.listen_addr,
        document_root = %config.server.document_root.display(),
        executor = %config.executor_type,
        mode = mode,
        "OxPHP HTTP server starting"
    );

    // Start dynamic worker scale manager if configured
    executor.start_scale_manager();

    // Initialize optional rate limiter
    let rate_limiter = if config.rate_limit > 0 {
        let limiter = Arc::new(server::rate_limit::RateLimiter::new(
            config.rate_limit,
            config.rate_window_seconds,
        ));
        tracing::info!(
            rate_limit = config.rate_limit,
            rate_window_seconds = config.rate_window_seconds,
            "Rate limiting enabled"
        );
        // Spawn background cleanup task
        let limiter_ref = Arc::clone(&limiter);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(60));
            loop {
                interval.tick().await;
                limiter_ref.cleanup();
            }
        });
        Some(limiter)
    } else {
        None
    };

    // Initialize optional TLS
    let tls_acceptor = match (&config.tls_cert, &config.tls_key) {
        (Some(cert), Some(key)) => {
            let acceptor = server::tls::load_tls_config(
                Path::new(cert),
                Path::new(key),
                config.tls_min_version,
            )?;
            tracing::info!(min_version = %config.tls_min_version, "TLS enabled");
            Some(acceptor)
        }
        _ => {
            // A half-configured pair is almost always a typo'd variable name.
            // Refuse to start rather than silently serve plain HTTP on a port
            // meant for HTTPS — fail-closed. Empty values count as unset, so
            // `${VAR:-}`-style substitutions of the whole pair still mean
            // "no TLS" rather than an error.
            if let Some(err) = config.half_configured_tls_error() {
                return Err(err.into());
            }
            if config.tls_min_version != oxphp::config::TlsMinVersion::V12 {
                tracing::warn!(
                    min_version = %config.tls_min_version,
                    "TLS_MIN_VERSION is set but TLS is not enabled — the floor has no effect"
                );
            }
            None
        }
    };

    // Initialize optional error pages
    let error_pages = match &config.error_pages_dir {
        Some(dir) => match server::error_pages::ErrorPages::load(Path::new(dir)) {
            Ok(pages) => {
                tracing::info!(dir = dir, "Custom error pages loaded");
                Some(Arc::new(pages))
            }
            Err(e) => {
                tracing::warn!(dir = dir, error = %e, "Failed to load error pages");
                None
            }
        },
        None => None,
    };

    // ── Register built-in event handlers ──

    // Always registered handlers
    dispatcher.on(handlers::request_id::RequestIdGenerator);
    dispatcher.on(handlers::metrics::MetricsRequestHandler::new(Arc::clone(
        &metrics,
    )));
    dispatcher.on(handlers::metrics::MetricsResponseHandler::new(Arc::clone(
        &metrics,
    )));
    dispatcher.on(handlers::server_header::ServerHeaderHandler);
    dispatcher.on(handlers::security_headers::SecurityHeadersHandler::new(
        &std::env::var("FRAME_OPTIONS").unwrap_or_else(|_| "SAMEORIGIN".to_string()),
    ));
    if config.access_log != oxphp::config::AccessLogLevel::Off {
        dispatcher.on(handlers::access_log::AccessLogHandler::new(
            config.access_log,
        ));
    }

    // Conditional handlers
    if let Some(ref limiter) = rate_limiter {
        dispatcher.on(handlers::rate_limit::RateLimitHandler::new(
            Arc::clone(limiter),
            Arc::clone(&metrics),
        ));
        tracing::info!("Rate limit handler registered");
    }
    if let Some(ref pages) = error_pages {
        dispatcher.on(handlers::error_pages::ErrorPagesHandler::new(Arc::clone(
            pages,
        )));
        tracing::info!("Error pages handler registered");
    }

    dispatcher.on(handlers::trace_context::TraceContextRequestHandler::new(
        config.trace_context,
    ));
    dispatcher.on(handlers::trace_context::TraceContextResponseHandler::new(
        config.trace_context,
    ));
    if config.trace_context {
        tracing::info!("Trace context handler registered");
    }

    if let Some(ref tp_config) = config.trusted_proxies {
        dispatcher.on(handlers::trusted_proxy::TrustedProxyHandler::new(Arc::new(
            tp_config.clone(),
        )));
        tracing::info!("Trusted proxy handler registered");
    }

    dispatcher.freeze();
    let dispatcher = Arc::new(dispatcher);
    let plugin_manager = Arc::new(plugin_manager);

    // Re-attach the listener bound in main() (before the privilege drop) to the
    // Tokio reactor. It is already non-blocking, as `from_std` requires.
    let listener = TcpListener::from_std(http_listener)?;
    let local_addr = listener.local_addr()?;

    tracing::info!(addr = %local_addr, "Server listening");

    let shutdown_flag = Arc::new(AtomicBool::new(false));

    // Spawn internal server if configured (before Server::new consumes
    // executor). The listener was bound in main(), before the privilege drop.
    let internal_handle = if let Some(internal_listener) = internal_listener {
        let metrics_ref = Arc::clone(&metrics);
        let config_ref = Arc::clone(&config);
        let executor_ref = Arc::clone(&executor);
        let pm_ref = Arc::clone(&plugin_manager);
        let shutdown_ref = Arc::clone(&shutdown_flag);
        Some(tokio::spawn(async move {
            if let Err(e) = server::internal::run_internal_server(
                internal_listener,
                metrics_ref,
                config_ref,
                executor_ref,
                pm_ref,
                shutdown_ref,
            )
            .await
            {
                tracing::error!(error = %e, "Internal server error");
            }
        }))
    } else {
        None
    };

    match (config.compression_level, config.gzip_level) {
        (0, _) => tracing::info!("Compression disabled"),
        (brotli, 0) => tracing::info!(brotli, "Compression enabled (brotli only)"),
        (brotli, gzip) => tracing::info!(brotli, gzip, "Compression enabled"),
    }

    if config.static_revalidate {
        tracing::info!(
            "Static file content cache: mtime revalidation enabled (STATIC_REVALIDATE=on)"
        );
    }

    if config.static_max_age.is_none() {
        tracing::info!("Cache-Control header for static files disabled (STATIC_MAX_AGE=off)");
    }

    tracing::info!(
        max_concurrent_streams = config.h2.max_concurrent_streams,
        max_pending_reset = config.h2.max_pending_accept_reset,
        max_header_list_bytes = config.h2.max_header_list_bytes,
        keepalive_interval_secs = config.h2.keepalive_interval.map(|d| d.as_secs()),
        keepalive_timeout_secs = config.h2.keepalive_timeout.as_secs(),
        "HTTP/2 limits"
    );

    let server = Arc::new(server::Server::new(
        &config.server,
        &config.h2,
        Arc::clone(&executor),
        Arc::clone(&metrics),
        dispatcher,
        tls_acceptor,
        server::compression::Levels {
            brotli: config.compression_level,
            gzip: config.gzip_level,
        },
        config.max_query_body,
        config.entry_file.clone(),
        config.worker_mode_enabled,
        config
            .static_max_age
            .map(|secs| format!("public, max-age={secs}")),
        config.static_revalidate,
        Arc::clone(&shutdown_flag),
    ));
    let semaphore = Arc::new(Semaphore::new(config.max_connections));

    // Notify plugins that server is ready
    plugin_manager.on_ready_all();

    // Spawn graceful shutdown handler
    let shutdown_notify = Arc::new(tokio::sync::Notify::new());
    let server_ref = Arc::clone(&server);
    let shutdown_ref = Arc::clone(&shutdown_notify);
    tokio::spawn(async move {
        shutdown_signal().await;
        tracing::info!("Received shutdown signal, draining connections");
        // Stop accepting, latch the drain flag, and wake every connection so
        // it winds down (GOAWAY on h2, Connection: close on idle h1
        // keep-alives). Long-lived streams are cancelled promptly by the
        // drain latch itself — the stream-flush path and the worker-mode
        // fiber sweep observe it; ordinary in-flight requests keep running
        // and get the whole drain window to finish. Requests still running
        // at the deadline are hard-cancelled by the drain loop below.
        server_ref.shutdown();
        shutdown_ref.notify_one();
    });

    // Accept-stall log state. A reported stall is bracketed by two lines — a
    // WARN when the loop parks, an INFO when the permit arrives — and both are
    // written from inside the park, so the pair is complete whether or not any
    // connection ever arrives afterwards. The WARN is held back when a recent
    // stall was already reported, so flapping around the ceiling cannot flood
    // the log; a stall passed over in silence gets no INFO either, and is
    // accounted for by `oxphp_accept_stalls_total` instead.
    const STALL_LOG_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
    let mut stall_warned = false;
    let mut last_stall_warn: Option<std::time::Instant> = None;

    // Accept loop
    loop {
        if server.is_shutdown() {
            break;
        }

        let accept_result = tokio::select! {
            result = listener.accept() => result,
            _ = shutdown_notify.notified() => break,
        };

        let (stream, remote_addr) = match accept_result {
            Ok(conn) => conn,
            Err(e) => {
                tracing::error!(error = %e, "Failed to accept connection");
                continue;
            }
        };

        // Fast path first: one non-atomic branch while permits are free. When
        // they are not, the loop is about to park with `stream` already
        // accepted and nothing new being served — from the outside that is
        // indistinguishable from a dead node (the health probe on
        // INTERNAL_ADDR does not go through this budget), so the state is
        // logged and counted before parking.
        let permit = match semaphore.clone().try_acquire_owned() {
            Ok(permit) => permit,
            Err(tokio::sync::TryAcquireError::Closed) => break, // shutting down
            Err(tokio::sync::TryAcquireError::NoPermits) => {
                metrics.accept_stall_begin();
                let parked_at = std::time::Instant::now();
                // A line per waiting connection would make the log its own
                // outage at high connection rates, so a recent report holds
                // the next one back. That suppression cannot be decided here
                // and then forgotten, though: nothing re-enters this branch
                // while the loop is parked, so a stall starting inside the
                // window would stay unreported for as long as it lasts — the
                // very silence this reports on. Wait out what is left of the
                // window against the permit instead, and report the stall if
                // it is still going when the window closes.
                let suppressed_for = last_stall_warn
                    .map(|warned_at| {
                        STALL_LOG_INTERVAL
                            .saturating_sub(parked_at.saturating_duration_since(warned_at))
                    })
                    .filter(|remaining| !remaining.is_zero());
                let acquire = semaphore.clone().acquire_owned();
                tokio::pin!(acquire);
                let acquired = match suppressed_for {
                    Some(remaining) => tokio::select! {
                        permit = &mut acquire => Some(permit),
                        _ = tokio::time::sleep(remaining) => None,
                    },
                    None => None,
                };
                let permit = match acquired {
                    Some(permit) => permit,
                    None => {
                        tracing::warn!(
                            max_connections = config.max_connections,
                            "MAX_CONNECTIONS exhausted, accept loop parked until a connection closes"
                        );
                        last_stall_warn = Some(std::time::Instant::now());
                        stall_warned = true;
                        acquire.await
                    }
                };
                metrics.accept_stall_end();
                let permit = match permit {
                    Ok(permit) => permit,
                    Err(_) => break, // semaphore closed — shutting down
                };
                // Written where the stall ends rather than on the next accept.
                // An overload usually subsides because the load went away, so
                // the next connection may be minutes later or never — a line
                // waiting for one would date the end of the stall to whenever
                // traffic happened to return, and on an instance stopped while
                // quiet it would never be written at all, leaving the log
                // saying the loop was parked until the process died.
                if stall_warned {
                    tracing::info!(
                        stalled_secs = parked_at.elapsed().as_secs_f64(),
                        "connection permits available again, accept loop resumed"
                    );
                    stall_warned = false;
                }
                permit
            }
        };

        let server_clone = Arc::clone(&server);
        tokio::spawn(async move {
            let _permit = permit; // held until task completes
            if let Err(e) = server_clone.handle_connection(stream, remote_addr).await {
                let msg = e.to_string();
                if msg.contains("timeout") {
                    tracing::warn!(
                        remote_addr = %remote_addr,
                        error = %e,
                        "Connection timeout"
                    );
                } else {
                    tracing::error!(
                        remote_addr = %remote_addr,
                        error = %e,
                        "Connection error"
                    );
                }
            }
        });
    }

    // Graceful drain: wait for in-flight connections to finish. Two phases:
    // until the deadline, in-flight requests run undisturbed (streams were
    // already cancelled by the drain latch); at the deadline, everything
    // still running is hard-cancelled and given a short beat to unwind its
    // bailout (shutdown handlers, final bytes) before the process exits.
    let active = server.active_connections();
    let active_in_flight = oxphp::php::worker_registry::total_in_flight();
    if active + active_in_flight > 0 {
        tracing::info!(
            active_connections = active,
            in_flight_requests = active_in_flight,
            "Draining in-flight connections"
        );
        let drain_deadline = tokio::time::Instant::now()
            + std::time::Duration::from_secs(config.drain_timeout_seconds);
        let hard_exit = drain_deadline + std::time::Duration::from_secs(2);
        let mut hard_cancelled = false;
        loop {
            let remaining = server.active_connections();
            // Requests that ended their response early (`oxphp_finish_request()`)
            // drop their connection while their background work keeps running on
            // the worker. Connections alone therefore report nothing left to wait
            // for: the drain does not cover that work at all — it is cut short
            // when the workers are torn down, never getting the window, and
            // nothing would bound it if it did survive.
            let in_flight = oxphp::php::worker_registry::total_in_flight();
            if remaining + in_flight == 0 {
                tracing::info!("All connections drained");
                break;
            }
            let now = tokio::time::Instant::now();
            if now >= drain_deadline && !hard_cancelled {
                hard_cancelled = true;
                tracing::warn!(
                    remaining_connections = remaining,
                    in_flight_requests = in_flight,
                    "Drain timeout reached, cancelling in-flight requests"
                );
                // Requests still waiting for a queue slot are in no worker, so
                // the sweep below cannot reach them. Close the gate first so
                // they are answered (503) inside the two-second unwind beat
                // instead of losing their connection when the runtime is
                // dropped.
                executor.close_admission();
                oxphp::php::worker_registry::hard_cancel_all(
                    oxphp::php::worker_registry::CancelReason::Shutdown,
                );
            }
            if now >= hard_exit {
                tracing::warn!(
                    remaining_connections = remaining,
                    in_flight_requests = in_flight,
                    "Forcing shutdown"
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    // Flush plugins after the (bounded) drain so RequestComplete / access-log
    // / APM events from requests finishing inside the drain window still
    // reach live plugins. Worst case this runs DRAIN_TIMEOUT_SECONDS + 2s
    // after SIGTERM — the orchestrator's grace period must exceed that.
    plugin_manager.shutdown_all();

    // Shutdown async worker pool
    if let Some(ref mut pool) = async_pool {
        pool.shutdown();
    }

    // Abort internal server task
    if let Some(handle) = internal_handle {
        handle.abort();
    }

    tracing::info!("Server stopped");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

/// Install signal handlers for SIGSEGV, SIGBUS, SIGABRT that write a minimal
/// diagnostic to stderr before re-raising. Fully signal-safe: only uses write(2),
/// no allocations, no std library calls that could deadlock.
///
/// Must be called AFTER php_module_startup() to override PHP's zend_signal handlers.
#[cfg(unix)]
fn install_crash_handlers() {
    use libc::{c_int, sigaction, SA_RESETHAND, SA_SIGINFO, SIGABRT, SIGBUS, SIGSEGV};
    use std::mem::MaybeUninit;

    extern "C" fn crash_handler(sig: c_int, info: *mut libc::siginfo_t, _ctx: *mut libc::c_void) {
        // STRICTLY signal-safe: only write(2) to stderr, then re-raise.
        // No allocations, no locks, no std::thread::current().
        unsafe {
            let stderr = 2;

            // Helper: write a u64 as hex to a fixed buffer, return slice
            fn u64_to_hex(val: u64, buf: &mut [u8; 18]) -> usize {
                buf[0] = b'0';
                buf[1] = b'x';
                let hex = b"0123456789abcdef";
                let mut v = val;
                let mut i = 18;
                if v == 0 {
                    buf[2] = b'0';
                    return 3;
                }
                while v > 0 && i > 2 {
                    i -= 1;
                    buf[i] = hex[(v & 0xf) as usize];
                    v >>= 4;
                }
                // Shift to start
                let len = 18 - i;
                for j in 0..len {
                    buf[2 + j] = buf[i + j];
                }
                2 + len
            }

            // Signal name
            let sig_name: &[u8] = match sig {
                libc::SIGSEGV => b"SIGSEGV",
                libc::SIGBUS => b"SIGBUS",
                libc::SIGABRT => b"SIGABRT",
                _ => b"SIG?",
            };

            libc::write(stderr, b"[CRASH] " as *const _ as _, 8);
            libc::write(stderr, sig_name.as_ptr() as _, sig_name.len());

            // Fault address from siginfo_t
            if !info.is_null() {
                let addr = (*info).si_addr() as u64;
                let code = (*info).si_code;
                let mut hex_buf = [0u8; 18];
                let hex_len = u64_to_hex(addr, &mut hex_buf);
                libc::write(stderr, b" addr=" as *const _ as _, 6);
                libc::write(stderr, hex_buf.as_ptr() as _, hex_len);
                let mut code_buf = [0u8; 4];
                let mut cn = if code < 0 {
                    (-code) as u32
                } else {
                    code as u32
                };
                let mut ci = code_buf.len();
                if cn == 0 {
                    ci -= 1;
                    code_buf[ci] = b'0';
                } else {
                    while cn > 0 && ci > 0 {
                        ci -= 1;
                        code_buf[ci] = b'0' + (cn % 10) as u8;
                        cn /= 10;
                    }
                }
                libc::write(stderr, b" code=" as *const _ as _, 6);
                if code < 0 {
                    libc::write(stderr, b"-" as *const _ as _, 1);
                }
                libc::write(stderr, code_buf[ci..].as_ptr() as _, (4 - ci) as _);
            }
            // Write thread ID (gettid syscall — signal safe, Linux only)
            #[cfg(target_os = "linux")]
            {
                let tid = libc::syscall(libc::SYS_gettid) as u64;
                let mut tid_buf = [0u8; 18];
                let tid_len = u64_to_hex(tid, &mut tid_buf);
                libc::write(stderr, b" tid=" as *const _ as _, 5);
                libc::write(stderr, tid_buf.as_ptr() as _, tid_len);
            }
            libc::write(stderr, b"\n" as *const _ as _, 1);

            // Persist to file
            let fd = libc::open(
                b"/tmp/oxphp-crash.log\0" as *const _ as _,
                libc::O_WRONLY | libc::O_CREAT | libc::O_APPEND,
                0o644,
            );
            if fd >= 0 {
                libc::write(fd, sig_name.as_ptr() as _, sig_name.len());
                if !info.is_null() {
                    let addr = (*info).si_addr() as u64;
                    let mut hex_buf = [0u8; 18];
                    let hex_len = u64_to_hex(addr, &mut hex_buf);
                    libc::write(fd, b" addr=" as *const _ as _, 6);
                    libc::write(fd, hex_buf.as_ptr() as _, hex_len);
                }
                #[cfg(target_os = "linux")]
                {
                    let tid = libc::syscall(libc::SYS_gettid) as u64;
                    let mut tid_buf = [0u8; 18];
                    let tid_len = u64_to_hex(tid, &mut tid_buf);
                    libc::write(fd, b" tid=" as *const _ as _, 5);
                    libc::write(fd, tid_buf.as_ptr() as _, tid_len);
                }
                libc::write(fd, b"\n" as *const _ as _, 1);
                libc::close(fd);
            }
            libc::raise(sig);
        }
    }

    for sig in [SIGSEGV, SIGBUS, SIGABRT] {
        unsafe {
            let mut sa = MaybeUninit::<sigaction>::zeroed().assume_init();
            sa.sa_flags = SA_SIGINFO | SA_RESETHAND;
            sa.sa_sigaction = crash_handler as *const () as usize;
            libc::sigemptyset(&mut sa.sa_mask);
            sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

#[cfg(not(unix))]
fn install_crash_handlers() {}
