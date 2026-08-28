//! What compressing a small response body costs, and which threads pay it.
//!
//! A body under the hand-off threshold is compressed on the async runtime's own
//! worker threads, and the runtime is half the width of the machine by default.
//! The workload here is fixed at one such body; the runtime width and the
//! ceiling on the blocking pool are the axes. Four numbers come out of each run:
//!
//! - **rate** — compressions per second at a fixed concurrency. Read across the
//!   width axis with everything else held still: if it scales with the width of
//!   the runtime, the ceiling is the runtime rather than the cost of the work.
//! - **neighbour** — how long a task that computes nothing waits between two
//!   turns of the runtime while those compressions run. It stands for
//!   everything the runtime carries that is not compressing: the accept loop,
//!   another connection's response, a health probe.
//! - **samples** — how many turns that probe got. It is a check on the line
//!   above, not a result: a probe that was scheduled a handful of times was
//!   starved, and its max and p99 describe the starvation rather than the wait.
//! - **single call** — one compression start to finish with nothing else on the
//!   runtime. It carries the per-call scaffolding with it — building the
//!   response, writing its headers, dropping it again — so where the code
//!   compresses inline, read it as an upper bound on the compression rather
//!   than as the compression alone. The scaffolding is identical on both sides,
//!   so the inline-to-hand-off difference is still the cost of handing off.
//!
//! Run: `cargo bench --no-default-features --bench compression_offload`

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::Bytes;
use http::{header, HeaderValue, Response};
use oxphp::server::compression::{maybe_compress, Coding};
use oxphp::types::full_body;

/// The size of a list endpoint's JSON response. It is the interesting size
/// precisely because it is unremarkable: it sits under every hand-off
/// threshold, at every level, for every coding.
const BODY_LEN: usize = 1_596;

/// What the server compresses dynamic responses with by default for a client
/// that accepts it.
const CODING: Coding = Coding::Zstd;
const CODING_NAME: &str = "zstd";
const LEVEL: i32 = 6;

/// In-flight compressions during the rate measurement.
const CONCURRENCY: usize = 64;

const WARMUP: Duration = Duration::from_millis(300);
const MEASURE: Duration = Duration::from_secs(2);

/// Sequential calls averaged for the single-call figure.
const SINGLE_CALLS: usize = 2_000;

/// How many times the whole config list is swept.
const PASSES: usize = 2;

/// Ceiling on recorded neighbour samples, so a runtime that schedules the probe
/// millions of times in two seconds cannot grow the vector without bound.
const NEIGHBOUR_SAMPLES: usize = 1 << 21;

fn main() {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);
    let body = json_body();
    assert_eq!(body.len(), BODY_LEN);

    println!("body {BODY_LEN} B, coding {CODING_NAME}, level {LEVEL}");
    println!("concurrency {CONCURRENCY}, {MEASURE:?} per run, machine has {cores} cores\n");

    // (runtime width, ceiling on the blocking pool). The pool ceiling only
    // bites where the work is handed off, and the point of asking is that
    // tokio's default is 512: enough for every in-flight request to become a
    // CPU-bound thread of its own.
    let mut configs = vec![(2, None), (2, Some(cores))];
    if cores > 2 {
        configs.push((cores, None));
    } else {
        println!("(machine is too narrow for the wide control run)\n");
    }

    // Every config is measured once per pass, and a pass sweeps the whole list.
    // Repeating a config twice in a row would show the spread of one moment and
    // hide drift across the run: the machine warms, another process starts or
    // stops, and all of that would land on whichever config happened to come
    // last — which, on a list ordered by runtime width, is the axis being read.
    let mut results: Vec<Vec<Run>> = (0..configs.len()).map(|_| Vec::new()).collect();
    for _ in 0..PASSES {
        for (i, &(width, max_blocking)) in configs.iter().enumerate() {
            results[i].push(measure(width, max_blocking, body.clone()));
        }
    }

    for (i, &(width, max_blocking)) in configs.iter().enumerate() {
        let pool = match max_blocking {
            Some(n) => format!("blocking pool ≤ {n}"),
            None => "blocking pool default (512)".to_string(),
        };
        println!("runtime width {width}, {pool}");
        for run in &results[i] {
            println!(
                "  rate {:>10.0}/s   neighbour max {:>7} µs p99 {:>6} µs ({} samples)   single {:>6.1} µs",
                run.rate,
                run.neighbour_max_us,
                run.neighbour_p99_us,
                run.neighbour_samples,
                run.single_us
            );
        }
        println!();
    }
}

struct Run {
    rate: f64,
    neighbour_max_us: u128,
    neighbour_p99_us: u128,
    neighbour_samples: usize,
    single_us: f64,
}

fn measure(width: usize, max_blocking: Option<usize>, body: Bytes) -> Run {
    // No IO or time driver is enabled: nothing on this path needs one, and the
    // measurement is about which threads run the compression.
    let mut builder = tokio::runtime::Builder::new_multi_thread();
    builder
        .worker_threads(width)
        // Mirrors the server's own runtime so the shape being measured is the
        // shape that ships.
        .thread_stack_size(512 * 1024)
        .global_queue_interval(32);
    if let Some(n) = max_blocking {
        builder.max_blocking_threads(n);
    }
    let rt = builder.build().expect("build runtime");

    rt.block_on(async move {
        drive(body.clone(), Instant::now() + WARMUP).await;

        let deadline = Instant::now() + MEASURE;
        let neighbour = tokio::spawn(neighbour_probe(deadline));
        let started = Instant::now();
        let done = drive(body.clone(), deadline).await;
        let elapsed = started.elapsed();
        let mut gaps = neighbour.await.expect("neighbour probe");

        gaps.sort_unstable();
        let neighbour_samples = gaps.len();
        let neighbour_max_us = gaps.last().map(|d| d.as_micros()).unwrap_or(0);
        let neighbour_p99_us = gaps
            .get(neighbour_samples.saturating_sub(1) * 99 / 100)
            .map(|d| d.as_micros())
            .unwrap_or(0);

        let single_started = Instant::now();
        for _ in 0..SINGLE_CALLS {
            let out = maybe_compress(response(body.clone()), CODING, LEVEL).await;
            std::hint::black_box(out);
        }
        let single_us = single_started.elapsed().as_secs_f64() * 1e6 / SINGLE_CALLS as f64;

        Run {
            rate: done as f64 / elapsed.as_secs_f64(),
            neighbour_max_us,
            neighbour_p99_us,
            neighbour_samples,
            single_us,
        }
    })
}

/// Compress from `CONCURRENCY` tasks until `deadline`, returning how many
/// bodies went through.
async fn drive(body: Bytes, deadline: Instant) -> u64 {
    let done = Arc::new(AtomicU64::new(0));
    let mut tasks = Vec::with_capacity(CONCURRENCY);
    for _ in 0..CONCURRENCY {
        let body = body.clone();
        let done = Arc::clone(&done);
        tasks.push(tokio::spawn(async move {
            // The clock read costs tens of nanoseconds against tens of
            // microseconds of compression, so it does not move the figure.
            while Instant::now() < deadline {
                let out = maybe_compress(response(body.clone()), CODING, LEVEL).await;
                std::hint::black_box(out);
                done.fetch_add(1, Ordering::Relaxed);
                // A connection yields between requests. Without this the loop
                // holds its worker thread for the whole run and the other
                // tasks — including the neighbour probe — never get polled at
                // all, which measures two busy threads rather than a runtime
                // carrying a workload.
                tokio::task::yield_now().await;
            }
        }));
    }
    for task in tasks {
        task.await.expect("compression task");
    }
    done.load(Ordering::Relaxed)
}

/// A task that wants nothing but its turn, recording how long each turn takes
/// to come back. It stands for everything the runtime carries that is not
/// compressing.
async fn neighbour_probe(deadline: Instant) -> Vec<Duration> {
    let mut gaps = Vec::with_capacity(1024);
    while Instant::now() < deadline {
        let waited = Instant::now();
        tokio::task::yield_now().await;
        if gaps.len() < NEIGHBOUR_SAMPLES {
            gaps.push(waited.elapsed());
        }
    }
    gaps
}

fn response(body: Bytes) -> Response<oxphp::types::ResponseBody> {
    let mut response = Response::new(full_body(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

/// A list endpoint's response, padded to exactly `BODY_LEN`. Only the size and
/// the redundancy matter — nothing here parses it.
fn json_body() -> Bytes {
    let mut s = String::from("{\"items\":[");
    let mut i = 0;
    loop {
        let record = format!(
            "{}{{\"id\":{i},\"sku\":\"AX-{i:04}\",\"name\":\"Widget {i}\",\"price\":{}.99,\"in_stock\":true}}",
            if i == 0 { "" } else { "," },
            10 + i
        );
        // Leave room for the padding record that lands the body on its size.
        if s.len() + record.len() + 32 > BODY_LEN {
            break;
        }
        s.push_str(&record);
        i += 1;
    }
    let head = format!("{}{{\"note\":\"", if i == 0 { "" } else { "," });
    let tail = "\"}]}";
    let fill = BODY_LEN - s.len() - head.len() - tail.len();
    s.push_str(&head);
    s.extend((0..fill).map(|n| char::from(b'a' + (n % 26) as u8)));
    s.push_str(tail);
    Bytes::from(s)
}
