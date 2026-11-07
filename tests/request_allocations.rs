//! How many heap allocations one keep-alive request costs on the dispatch
//! side, counted on the real connection path with the stub executor.
//!
//! Each figure is a ceiling half an allocation over what the path costs: one
//! more allocation per request fails the test, so it has to be chosen rather
//! than slip in. Lower a ceiling when the path gets cheaper.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use oxphp::events::EventDispatcher;
use oxphp::handlers;
use oxphp::metrics::Metrics;

struct CountingAlloc;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Set on the thread playing the client, whose own allocations are not
    /// the server's cost. `const` so reading it cannot itself allocate.
    static NOT_COUNTED: Cell<bool> = const { Cell::new(false) };
}

fn count() {
    if !NOT_COUNTED.try_with(Cell::get).unwrap_or(true) {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: every call forwards to `System` with the arguments it was given; the
// counter neither allocates nor touches the memory.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        System.alloc(layout)
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        System.alloc_zeroed(layout)
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        count();
        System.realloc(ptr, layout, new_size)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        System.dealloc(ptr, layout)
    }
}

#[global_allocator]
static COUNTING: CountingAlloc = CountingAlloc;

/// The handlers `main` registers on a default configuration.
fn default_handlers(metrics: &Arc<Metrics>, trace_context: bool) -> EventDispatcher {
    let mut dispatcher = EventDispatcher::new();
    dispatcher.on(handlers::request_id::RequestIdGenerator);
    dispatcher.on(handlers::metrics::MetricsRequestHandler::new(Arc::clone(
        metrics,
    )));
    dispatcher.on(handlers::metrics::MetricsResponseHandler::new(Arc::clone(
        metrics,
    )));
    dispatcher.on(handlers::server_header::ServerHeaderHandler);
    dispatcher.on(handlers::security_headers::SecurityHeadersHandler::new(
        "SAMEORIGIN",
    ));
    dispatcher.on(handlers::trace_context::TraceContextRequestHandler::new(
        trace_context,
    ));
    dispatcher.on(handlers::trace_context::TraceContextResponseHandler::new(
        trace_context,
    ));
    dispatcher
}

/// Reads one response off a keep-alive connection, up to the end of its body.
fn read_response(sock: &mut TcpStream, buf: &mut Vec<u8>) {
    buf.clear();
    let mut chunk = [0u8; 4096];
    loop {
        let n = sock.read(&mut chunk).unwrap();
        assert_ne!(n, 0, "server closed the connection mid-response");
        buf.extend_from_slice(&chunk[..n]);
        let Some(head_end) = buf.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
        let length: usize = head
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .map_or(0, |v| v.trim().parse().unwrap());
        if buf.len() >= head_end + 4 + length {
            return;
        }
    }
}

/// `request` with `{etag}` replaced by the tag the server gives `/style.css`.
fn with_current_etag(addr: SocketAddr, request: &[u8]) -> Vec<u8> {
    let request = std::str::from_utf8(request).unwrap();
    if !request.contains("{etag}") {
        return request.as_bytes().to_vec();
    }
    let mut sock = TcpStream::connect(addr).unwrap();
    sock.write_all(STATIC_REQUEST).unwrap();
    let mut response = Vec::new();
    read_response(&mut sock, &mut response);
    let response = String::from_utf8_lossy(&response);
    let (_, etag) = response
        .lines()
        .filter_map(|line| line.split_once(": "))
        .find(|(name, _)| name.eq_ignore_ascii_case("etag"))
        .unwrap();
    request.replace("{etag}", etag).into_bytes()
}

/// Allocations per request over a warmed-up keep-alive connection. The first
/// requests pay for what lives as long as the connection or the process.
fn allocations_per_request(addr: SocketAddr, request: Vec<u8>, status: u16) -> f64 {
    const WARM_UP: u32 = 200;
    const MEASURED: u32 = 1_000;

    let counted = std::thread::spawn(move || {
        NOT_COUNTED.with(|flag| flag.set(true));
        let mut sock = TcpStream::connect(addr).unwrap();
        sock.set_nodelay(true).unwrap();
        let mut buf = Vec::with_capacity(16 * 1024);
        for _ in 0..WARM_UP {
            sock.write_all(&request).unwrap();
            read_response(&mut sock, &mut buf);
        }
        // A request that misses its path would be measuring another one.
        let expected = format!("HTTP/1.1 {status} ");
        assert!(
            buf.starts_with(expected.as_bytes()),
            "expected {expected:?}, got {:?}",
            String::from_utf8_lossy(&buf[..buf.len().min(40)])
        );
        let before = ALLOCATIONS.load(Ordering::Relaxed);
        for _ in 0..MEASURED {
            sock.write_all(&request).unwrap();
            read_response(&mut sock, &mut buf);
        }
        ALLOCATIONS.load(Ordering::Relaxed) - before
    })
    .join()
    .unwrap();
    counted as f64 / f64::from(MEASURED)
}

async fn measure(case: &Case) -> f64 {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::write(dir.path().join("index.php"), "<?php\n").unwrap();
    std::fs::write(dir.path().join("style.css"), "body { color: red }\n").unwrap();
    std::fs::write(dir.path().join("big.bin"), vec![0u8; PAST_THE_FILE_CACHE]).unwrap();
    let metrics = Arc::new(Metrics::new());
    let dispatcher = default_handlers(&metrics, case.trace_context);
    let (addr, _server) = common::start_test_server(
        dir.path(),
        &oxphp::config::H2Config::default(),
        case.entry,
        metrics,
        dispatcher,
    )
    .await;
    let (request, status) = (case.request, case.status);
    tokio::task::spawn_blocking(move || {
        allocations_per_request(addr, with_current_etag(addr, request), status)
    })
    .await
    .unwrap()
}

/// Allocations of a server with no traffic, with the plugin timers that run for
/// applications that never use what they watch ticking far faster than they do
/// in production.
async fn allocations_while_idle() -> u64 {
    let dir = tempfile::TempDir::new().unwrap();
    let metrics = Arc::new(Metrics::new());
    let dispatcher = default_handlers(&metrics, false);
    let (_addr, _server) = common::start_test_server(
        dir.path(),
        &oxphp::config::H2Config::default(),
        None,
        metrics,
        dispatcher,
    )
    .await;

    #[cfg(feature = "plugin-shared")]
    {
        use oxphp::plugins::ox_shared::config::{LockDiagnosticsLevel, SharedConfig};
        use oxphp::plugins::ox_shared::{deadlock, eviction, registry};
        registry::init_registry(SharedConfig {
            enabled: true,
            max_entries: 1_000,
            max_bytes: 64 * 1024,
            metrics_enabled: false,
            introspection_enabled: false,
            introspection_preview_enabled: false,
            cycle_detect_depth: 16,
            cycle_detect_edges: 10_000,
            max_value_size: 1 << 20,
            max_channel_bytes: 64 << 20,
            lock_diagnostics: LockDiagnosticsLevel::Off,
            lock_poll_interval_ms: 100,
            preview_string_limit: 256,
            preview_array_limit: 20,
        });
        deadlock::start_detector(LockDiagnosticsLevel::Warn, TIMER_TICK);
        eviction::start_scheduler(TIMER_TICK);
    }

    // Whatever the requests before this one left behind settles first.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let before = ALLOCATIONS.load(Ordering::Relaxed);
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    ALLOCATIONS.load(Ordering::Relaxed) - before
}

const SCRIPT_REQUEST: &[u8] = b"GET /index.php?page=2&sort=name HTTP/1.1\r\n\
Host: example.test\r\n\
User-Agent: Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/128.0\r\n\
Accept: text/html,application/xhtml+xml\r\n\
Accept-Language: en-US,en;q=0.5\r\n\
Accept-Encoding: gzip, deflate, br\r\n\
Cookie: session=abc123; theme=dark\r\n\
\r\n";

const STATIC_REQUEST: &[u8] = b"GET /style.css HTTP/1.1\r\n\
Host: example.test\r\n\
User-Agent: Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/128.0\r\n\
Accept: text/css,*/*;q=0.1\r\n\
Accept-Encoding: gzip, deflate, br\r\n\
\r\n";

/// Revalidates the plain copy of a cached file.
const NOT_MODIFIED_REQUEST: &[u8] = b"GET /style.css HTTP/1.1\r\n\
Host: example.test\r\n\
User-Agent: Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/128.0\r\n\
Accept: text/css,*/*;q=0.1\r\n\
If-None-Match: {etag}\r\n\
\r\n";

/// Revalidates a compressed copy, whose tag the server weakened.
const WEAK_NOT_MODIFIED_REQUEST: &[u8] = b"GET /style.css HTTP/1.1\r\n\
Host: example.test\r\n\
User-Agent: Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/128.0\r\n\
Accept: text/css,*/*;q=0.1\r\n\
If-None-Match: W/{etag}\r\n\
\r\n";

/// Revalidates a file the cache refuses, so it is answered off the disk.
const UNCACHED_NOT_MODIFIED_REQUEST: &[u8] = b"GET /big.bin HTTP/1.1\r\n\
Host: example.test\r\n\
User-Agent: Mozilla/5.0 (X11; Linux x86_64) Gecko/20100101 Firefox/128.0\r\n\
Accept: */*\r\n\
If-None-Match: *\r\n\
\r\n";

/// Over the largest file the content cache keeps.
const PAST_THE_FILE_CACHE: usize = 2 * 1024 * 1024;

#[cfg(feature = "plugin-shared")]
const TIMER_TICK: std::time::Duration = std::time::Duration::from_millis(5);

const ROUTED_REQUEST: &[u8] = b"GET /users/42 HTTP/1.1\r\n\
Host: example.test\r\n\
User-Agent: x\r\n\
Accept: */*\r\n\
\r\n";

struct Case {
    name: &'static str,
    request: &'static [u8],
    entry: Option<&'static str>,
    trace_context: bool,
    status: u16,
    ceiling: f64,
}

// One test, because the counter is process-wide and measurements cannot overlap.
#[tokio::test(flavor = "multi_thread")]
async fn allocations_per_request_stay_under_their_ceilings() {
    let cases = [
        Case {
            name: "script",
            request: SCRIPT_REQUEST,
            entry: None,
            trace_context: false,
            status: 200,
            ceiling: 16.5,
        },
        Case {
            name: "routed script",
            request: ROUTED_REQUEST,
            entry: Some("index.php"),
            trace_context: false,
            status: 200,
            ceiling: 16.5,
        },
        Case {
            name: "traced script",
            request: SCRIPT_REQUEST,
            entry: None,
            trace_context: true,
            status: 200,
            ceiling: 22.5,
        },
        Case {
            name: "cached static file",
            request: STATIC_REQUEST,
            entry: None,
            trace_context: false,
            status: 200,
            ceiling: 8.5,
        },
        Case {
            name: "304 on a cached file",
            request: NOT_MODIFIED_REQUEST,
            entry: None,
            trace_context: false,
            status: 304,
            ceiling: 10.5,
        },
        Case {
            name: "304 on a cached compressed copy",
            request: WEAK_NOT_MODIFIED_REQUEST,
            entry: None,
            trace_context: false,
            status: 304,
            ceiling: 10.5,
        },
        Case {
            name: "304 on a file past the cache",
            request: UNCACHED_NOT_MODIFIED_REQUEST,
            entry: None,
            trace_context: false,
            status: 304,
            ceiling: 25.5,
        },
    ];
    for case in cases {
        let measured = measure(&case).await;
        println!("{}: {measured:.2}", case.name);
        assert!(
            measured <= case.ceiling,
            "{}: {measured:.2} allocations per request, ceiling {}",
            case.name,
            case.ceiling
        );
    }

    let idle = allocations_while_idle().await;
    println!("idle: {idle}");
    assert_eq!(idle, 0, "an idle server allocated {idle} times");
}
