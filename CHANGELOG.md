# Changelog

All notable changes to OxPHP are documented in this file.

## [0.5.0] - 2026-05-05

Headline work since `v0.4.0`: a **canonical entry-script + worker-mode model** (`ENTRY_FILE` / `WORKER_MODE_ENABLED` retiring `INDEX_FILE` / `WORKER_FILE`), a new `OxPHP\Server\Worker` class for runtime introspection and application-driven recycling via `Worker::scheduleExit()`, strict parsing of boolean and `STATIC_MAX_AGE` env vars, and a clearer `STATIC_MAX_AGE` / `STATIC_REVALIDATE` rename for the static-file cache.

### Added

- New PHP class `OxPHP\Server\Worker` — unified runtime handle for worker introspection. Methods: `current`, `isWorkerMode`, `getId`, `getStartTime`, `getRequestCount` (1-based count of requests handled by this OS thread; grows in both modes since traditional reuses persistent threads), `getMemoryUsage`, `getRss`, `getMaxMemoryBytes`, `scheduleExit`, `isExitScheduled`, `getExitReason`, `serve`. Available in both traditional and worker modes. See `docs/en/php/worker-class.md`.
- New PHP exception `OxPHP\Server\Exception\InvalidServeContextException`, thrown by `Worker::serve()` when called outside worker mode.
- `Worker::scheduleExit()` — application-driven worker recycling. Marks the current worker for graceful exit after the active request completes; the supervisor respawns a fresh worker, re-running the outer scope. Companion methods `Worker::isExitScheduled()` and `Worker::getExitReason()` expose the pending exit state. No-op in traditional mode.
- Environment variables `ENTRY_FILE` and `WORKER_MODE_ENABLED` — single canonical entry script plus an explicit worker-mode toggle. `ENTRY_FILE` selects the routing mode by extension (unset = direct mapping, `*.php` = front controller, non-`.php` = SPA fallback). When `WORKER_MODE_ENABLED=true`, `ENTRY_FILE` must point at a `.php` script and the server runs persistent workers. Resolution accepts relative paths (against `DOCUMENT_ROOT`, including `..`) and absolute paths. The startup `mode_decided` log line records which combination was selected. See `docs/en/operations/configuration.md`.

### Changed

- `oxphp_worker_recycles_by_reason_total{reason="max_requests"}` Prometheus label is renamed to `reason="scheduled"` to reflect that the recycle reason is now driven by `Worker::scheduleExit()` instead of an automatic request counter.
- `/config` endpoint now reports `entry_file` and `worker_mode_enabled` in place of `index_file`, `worker_file`, and the synthetic `worker_mode` boolean.
- Static file cache environment variables renamed for clarity: `STATIC_CACHE_TTL` → `STATIC_MAX_AGE` (the value is the `Cache-Control: max-age` it sets), and `STATIC_CACHE` → `STATIC_REVALIDATE` with the polarity flipped (`STATIC_REVALIDATE=on` enables mtime revalidation; previously `STATIC_CACHE=off` did the same thing). Defaults are unchanged: 30 days `max-age`, no revalidation. `/config` reports `static_max_age` and `static_revalidate` in place of `static_cache_ttl` and `static_cache_enabled`.
- **BREAKING:** `STATIC_MAX_AGE` (and the deprecated `STATIC_CACHE_TTL`) are now strictly parsed: garbage values like `STATIC_MAX_AGE=garbage` fail at startup with an error naming the variable, where they previously silently fell back to a missing `Cache-Control` header. Empty assignments (`STATIC_MAX_AGE=`) and unset variables still fall back to the default (30 days), matching the bool-parser policy.
- **BREAKING:** boolean environment variables are now strictly parsed against a fixed canonical set (`on`/`true`/`1`/`yes` for truthy, `off`/`false`/`0`/`no` for falsy, case-insensitive and trimmed). Any non-empty value outside that set — including typos like `ture` — fails fast at startup with an error naming the variable, rather than silently defaulting. An unset variable or empty assignment (`FOO=`) falls back to the default; this matches Docker Compose / Kubernetes substitution like `FOO=${FOO}` when the host variable is missing. Affected variables: `WORKER_MODE_ENABLED`, `STATIC_REVALIDATE`, `TRACE_CONTEXT`, `SUPERGLOBALS_ENABLED`, `SHARED_ENABLED`, `SHARED_METRICS_ENABLED`, `SHARED_INTROSPECTION_ENABLED`, `SHARED_INTROSPECTION_PREVIEW_ENABLED`, `SHARED_POISON_STRICT`, `PROFILER_ENABLED`, `PROFILER_INTERNAL`, `PROFILER_EXPORT_XHGUI`. The legacy `STATIC_CACHE` compatibility shim remains intentionally lenient (only `off` enables revalidation, anything else disables). Audit any deployment that relied on non-canonical bool values like `enabled` — these now refuse to start.

### Deprecated

- Environment variable `WORKER_MAX_REQUESTS` — parsed and ignored; emits a `WARN` log line at startup if set. Migrate to `WORKER_MAX_MEMORY_MIB` for safety-net recycling, or to `Worker::scheduleExit()` for application-driven recycling. Will be removed entirely in a subsequent release.
- Environment variables `INDEX_FILE` and `WORKER_FILE` — still parsed for backwards compatibility; emit a `WARN` log line at startup and map onto the new model: `INDEX_FILE=...` ≡ `ENTRY_FILE=...`, and `WORKER_FILE=...` ≡ `WORKER_MODE_ENABLED=true ENTRY_FILE=...`. When both legacy and new variables are set, the new ones win and the warning still fires. The legacy forms will be removed in a subsequent release.
- Environment variables `STATIC_CACHE_TTL` and `STATIC_CACHE` — still parsed for backwards compatibility; emit a `WARN` log line at startup and map onto the new model: `STATIC_CACHE_TTL=...` ≡ `STATIC_MAX_AGE=...`, and `STATIC_CACHE=off` ≡ `STATIC_REVALIDATE=on`. When both legacy and new variables are set, the new ones win and the warning still fires. The legacy forms will be removed in a subsequent release.

### Internal

- New benchmark tooling under `scripts/`: `bench-wrk.sh` (one-shot wrk runner against a configurable target) and `sweep-config.sh` (matrix sweep over `TOKIO_WORKERS` × `PHP_WORKERS` for tuning). Not wired into CI; local-only.
- Bump dependencies for the post-`0.4.0` cycle: `opentelemetry` / `opentelemetry_sdk` / `opentelemetry-otlp` / `opentelemetry-semantic-conventions` `0.27 → 0.31`, `tonic` `0.12 → 0.14` (now requires the explicit `grpc-tonic` feature on `opentelemetry-otlp`), `rand` `0.8 → 0.10`, `getrandom` `0.2 → 0.4`, `reqwest` `0.12 → 0.13`, `brotli` `7 → 8`, `lru` `0.12 → 0.18`. No user-visible behavior change; OTel migration switches to `SdkTracerProvider` with `with_batch_exporter`, `Resource::builder()`, and the new `force_flush()` `Result` shape, and `rand`/`getrandom` call sites move to the `Rng::random` / `getrandom::fill` APIs.

## [0.4.0] - 2026-05-02

Headline work since `v0.3.0`: **PHP 8.5 support** (opt-in via `:php8.5*` tags, `latest` still resolves to 8.4 pending soak), and a chain of fiber / `Shared\*` / streaming bugfixes that surfaced once worker-mode `Channel`/`Map`/`Pool` traffic and `oxphp_async()` workers got real exercise.

### Added

- PHP 8.5 support. Build pipeline now produces `:php8.5`, `:php8.5-alpine{X.Y}`, and patch-pinned `:0.X.Y-php8.5.Z-alpine{X.Y}` tags alongside the existing 8.4 ones. `latest` and unsuffixed `{ver}` continue to resolve to PHP 8.4 in this release; the default flip to 8.5 lands in a follow-up release after a soak window. To opt in early, pull `:php8.5` (or any `*-php8.5*` variant).
- `SAPI_HEADER_DELETE_PREFIX` support (PHP 8.5.6+) — `header_remove('X-Foo-')` now strips every previously-set header whose `"{name}: {value}"` line case-insensitively starts with the given prefix, matching upstream SAPI behavior. PHP < 8.5.6 keeps the existing exact-match semantics.

### Fixed

- Streaming responses on the traditional executor losing chunked `Transfer-Encoding` after the first `oxphp_stream_flush` on a worker. The bridge per-request context (`stream_mode` / `headers_sent` / `finished`) is `__thread`-local and was leaking across requests; the traditional path now resets it before each request to match worker mode.
- `oxphp_bridge_in_fiber()` misreporting fiber context — both on the main thread (where PHP seeds `EG(current_fiber_context)` to `EG(main_fiber_context)` at request startup) and inside a user-level `Fiber::start()` body (which installs its own `zend_fiber_context` distinct from main). The latter caused `Shared\Channel::recv()` / `send()` inside a user Fiber to take the fiber-suspend path, hit rc=1 from `oxphp_bridge_fiber_await` ("not in oxphp fiber"), and surface as `RuntimeException: recv: fiber_await rc=1`. The predicate now keys off the SAPI's private `oxphp_current_fiber` __thread pointer via a registered callback (`oxphp_bridge_set_in_fiber_check`), which is the only authoritative source for "is this thread inside an oxphp scheduler fiber". User fibers correctly fall through to the thread-blocking branch.
- `DELETE /__profiler/runs/{id}` panicking the worker with "cannot block from within a runtime". The internal HTTP server dispatches sync handlers from inside hyper's async service, so the index-lock acquire switched from `blocking_lock()` to a `try_lock` retry loop with a 5 s deadline that degrades to `503` on real contention.
- Plugin-defined classes, interfaces, enums, and free functions advertised no parameter names, so PHP named-argument syntax (e.g. `$ch->send('x', timeout: 0.1)`) failed with "Unknown named parameter" and `ReflectionParameter::getName()` returned an empty list. The bridge method/function registration now carries per-parameter name/type/optional arrays, and the SAPI extension synthesizes a full `zend_internal_arg_info` array with real names instead of an unnamed return-only stub.
- `OxPHP\Shared\Channel`, `Shared\Map`, and `Shared\Pool` handles arriving as `null` on the receiving worker thread when captured by an `oxphp_async()` closure (via `use ($var)` or as a variadic arg). The cross-thread wrapper rebuild in the C bridge listed only `Counter`/`Flag`/`Once`/`Mutex` in its tag→class switch, so the other three `SharedType` variants fell through to `default` and produced `IS_NULL`. The mapping now lives only in Rust (`SharedType::php_class_cstr`) and the C bridge calls a weak-linked `oxphp_shared_class_name(type_tag)` export, so adding a `SharedType` variant cannot drift again. Re-enables the `shared/test_channel_fiber_*` worker-mode suites.

### Internal

- `src/php/bindings.rs` split into `src/php/bindings/{common.rs, v8_4.rs, v8_5.rs}` with cfg-selected per-version modules. `build.rs` detects the linked PHP via `php-config --vernum` (or `PHP_VERSION_ID` env override).
- `release.yml` gains a `php-suite` pre-publish gate running a focused subset of `tests/run_all.sh` (`headers, cookies, get_post_request, input, pathinfo, errors`) against both 8.4 and 8.5 images before manifest creation. Failure aborts the publish entirely.
- `weekly-rebuild.yml` gains skip-if-unchanged via upstream digest annotation comparison plus the same focused subset suite job, gated on whether any matrix cell actually rebuilt.

## [0.3.0] - 2026-04-22

Headline work since `v0.2.0`: **shared state for PHP workers without Redis** (seven `OxPHP\Shared\*` primitives), a per-request **PHP profiler**, **APM auto-instrumentation**, trusted-proxy and Kubernetes integrations for production deployments, security-header hardening, and a cosign-signed parametrized Docker image matrix.

### Added

#### `OxPHP\Shared\*` primitives

See the [Shared State overview](docs/en/features/shared-state.md) for the concept and mental model, and the per-type docs for API reference, runnable examples, and gotchas.

- [`Shared\Counter`](docs/en/features/shared-counter.md) — atomic int64 with `inc` / `dec` / `add` / `compareAndSet` / `addBatch` / `reset`.
- [`Shared\Flag`](docs/en/features/shared-flag.md) — atomic bool with `test` / `set` / `clear` / `exchange` / `compareAndSet`.
- [`Shared\Once`](docs/en/features/shared-once.md) — run-once container with `init(factory)` / `trySet` / `get`. Reentrant `init` throws `DeadlockException`.
- [`Shared\Mutex`](docs/en/features/shared-mutex.md) — poisoning mutex guarding a stored value. `with(callable, timeout)` and `tryWith(callable)` scope-guard the critical section; poisoning isolates failed-mid-update state.
- [`Shared\Channel`](docs/en/features/shared-channel.md) — bounded MPMC queue with fiber-aware `send` / `recv`. `sendMany` / `recvMany` for batching.
- [`Shared\Map`](docs/en/features/shared-map.md) — concurrent `string → mixed` store with `get` / `set` / `update` / `getOrSet` / `setIfAbsent` / batched `setMany` / `getMany` / `removeMany`. Per-instance cap via `maxEntries`.
- [`Shared\Pool`](docs/en/features/shared-pool.md) — bounded object pool with lazy factory, optional destroy callback, strict `maxSize` budget, per-thread affinity, and idle-timeout eviction. `with($body)` scope-guards acquire/release.

#### Shared-registry observability

See [Shared Observability](docs/en/operations/shared-observability.md) for the operator's reference.

- Internal-server endpoints: `/__ox_shared/summary`, `/entries`, `/entry?id=…`, `/preview?id=…`, `/types`, `/graph?id=…` for live registry introspection.
- Prometheus metrics under `oxphp_shared_*` — aggregate-per-type (`objects_total`, `operations_total`, `bytes`, `capacity_saturation`) plus per-instance for Channel / Map / Pool.
- Cross-thread deadlock detector — `oxphp_shared_deadlock_detected_total` ticks when the wait-for scanner finds a mutex cycle.
- Shared `preview` previews are gated behind `SHARED_INTROSPECTION_PREVIEW_ENABLED` so production deployments can disable value exposure without losing shape counts.

#### Profiling

- Per-request PHP profiler (`plugin-profiler` feature — now part of the default Cargo feature set; `-DOXPHP_WITH_PROFILER=1` propagated to both C build stages) with four output formats: xhprof, speedscope, pprof, collapsed.
- PHP SDK: `OxPHP\Profile\{start, stop, pause, resume, mark, metric, is_active}` functions.
- Seven PHP attributes: `#[Profile]`, `#[Exclude]`, `#[Sample]`, `#[Tag]`, `#[Mark]`, `#[SlowThreshold]`, `#[MemoryThreshold]`.
- Trigger modes: cookie (`OXPROF=<token>`), header (`X-OxPHP-Profile: <token>`), query (`?__oxprof=<token>`), and statistical (`PROFILER_SAMPLE_RATE`).
- In-memory LRU cache (`PROFILER_RETENTION_COUNT`) + disk retention with background trimmer (5-second cadence, atomic rename).
- Token-bucket disk write rate limiting (`PROFILER_DISK_MAX_PER_SEC`).
- HTTP push (`PROFILER_EXPORT_URL`) with 3× exponential backoff retry, 5 s wallclock cap, bearer-token auth, xhgui envelope auto-detect.
- Internal HTTP routes at `/__profiler/` — list, metadata, raw format download, speedscope redirect, DELETE, config, stats — with optional bearer-token auth and path-traversal revalidation.
- Prometheus metrics: `oxphp_profiler_runs_total{source}`, `spans_collected_total`, `bytes_written_total{format}`, `disk_drops_total`, `http_push_failures_total`, `truncated_total`, `in_memory_runs`.
- `xhgui` Docker test profile demonstrating the full push → mongo → xhgui UI flow.
- Per-locale documentation at `docs/{en,ru,zh}/features/profiling.md`.

#### APM & tracing

- APM plugin (`plugin-apm`) with auto-instrumentation, PHP tracing SDK, and error capture.
- Plugin PHP builder API for registering Rust-backed PHP functions and classes from plugins. Async and APM subsystems migrated from the C extension into Rust plugins.
- Return-type support in the C builder API for plugin methods.

#### HTTP & routing

- **Trusted proxy support** via `TRUSTED_PROXIES` — accepts trusted-proxy CIDR list (or `private`), processes RFC 7239 `Forwarded` and `X-Forwarded-*` headers, and overrides `REMOTE_ADDR`, `HTTPS`, `REQUEST_SCHEME`, `SERVER_NAME`, `SERVER_PORT` for PHP using the rightmost-non-trusted algorithm.
- **Kubernetes health probes** at `/readyz` and `/livez` with graceful-shutdown awareness.
- `PATH_INFO` splitting via `SPLIT_PATH_INFO_ENABLED` — nginx/PHP-FPM-style front-controller routing.
- `PHP_DENY_DIRS` env var to block `.php` execution in specified paths.
- Dot-path access blocked by default, with an RFC 8615 `.well-known` exception.

#### Security headers

- `X-Content-Type-Options: nosniff` on all responses.
- Configurable `X-Frame-Options` (`FRAME_OPTIONS`, default `SAMEORIGIN`) for clickjacking protection.

#### Operations

- CLI argument parsing: `--help`, `--version`, `--config --check`.
- Startup errors emitted as structured JSON logs (previously plain text).
- Docker `HEALTHCHECK` wired into `compose.yml`.

#### Supply chain & packaging

- Parametrized Docker image matrix: two Dockerfiles (dev + `Dockerfile.alpine-release`) sharing `ARG PHP_VERSION` / `ARG ALPINE_VERSION` / `ARG BASE_IMAGE`.
- Canonical minor-floating (`{ver}-php{minor}-alpine{alpine}`) and patch-pinned tags published to `ghcr.io/oxphp/oxphp`, plus aliases (`php{minor}`, `latest`, etc.).
- cosign-signed release images via GitHub OIDC.
- Weekly rebuild workflow re-publishes canonical tags with fresh upstream PHP patches and re-signs.
- Prod image now ships `php` CLI, `docker-php-ext-install`, `phpize`, and `www-data` out of the box (was bare alpine in 0.2.0).

#### Testing

- PHP integration test suite — 186 tests across 21 groups and 12 Docker profiles, covering apm, async, errors, framework, pathinfo, ratelimit, TLS, timeout, worker and more.

#### Configuration

All Shared-state tunables are read at startup via the `SHARED_*` env-var prefix (fallbacks to `OX_SHARED_*` and bare keys). See [Shared State → Configuration](docs/en/features/shared-state.md#configuration) for the full table. Highlights:

- `SHARED_MAX_ENTRIES` (default 100 000) / `SHARED_MAX_BYTES` (default 1 GiB) — global caps.
- `SHARED_CYCLE_DETECT_DEPTH` (16) / `SHARED_CYCLE_DETECT_EDGES` (10 000) — cycle-check walker bounds.
- `SHARED_INTROSPECTION_ENABLED` / `SHARED_METRICS_ENABLED` — per-feature kill switches.
- `SHARED_LOCK_DIAGNOSTICS` (`off` / `warn` / `strict`) — escalates reentry / deadlock signals.

#### Rust plugin-author API

- `MapInner::retain<F>` — exposes `DashMap::retain` with proper refcount release for nested `SharedValue::Shared` targets. Lets plugin authors prune a map in a single shard-walk instead of the N-lock `keys()`+`remove()` pattern.

#### Documentation

- [`docs/en/features/shared-state.md`](docs/en/features/shared-state.md) — overview, mental model, type-selection matrix, canonical hand-rolled-counter → `Shared\*` migration example.
- Per-type docs for all seven Shared\* v1 types (see list above).
- [`docs/en/operations/shared-observability.md`](docs/en/operations/shared-observability.md) — introspection endpoints, Prometheus catalogue, diagnostic playbooks.
- [`docs/en/features/migrating-to-external-store.md`](docs/en/features/migrating-to-external-store.md) — when and how to promote `Shared\*` state to Redis / NATS / Kafka.

#### Tooling

- `tests/soak/pool_soak.sh` + `tests/soak/workload.php` — manual (non-CI) 24h soak harness for pre-release Shared\Pool stability sign-off. Not wired into `tests/run_all.sh`; [invocation notes in the observability doc](docs/en/operations/shared-observability.md#long-running-soak-harness).

### Changed

- **Routing refactored** into per-mode modules with a performance and behavior overhaul.
- **Request latency reduced across all stack layers** — hot-path allocations, routing, and response assembly.
- `oxphp_request_heartbeat($time)` now also resets PHP's own `max_execution_time` timer to `$time` seconds alongside the server-side deadline. Previously only the server deadline was extended, so long-running scripts could still be killed by Zend's "Maximum execution time exceeded" fatal even after a heartbeat. Scripts that opted out of the PHP timer via `set_time_limit(0)` or `max_execution_time=0` are left alone — the heartbeat does not re-enable a disabled timer.
- Welcome page redesigned as a minimal "is running" status page.
- **Prod image `USER` policy**: `Dockerfile.alpine-release` no longer sets a final `USER` — matches `nginx:alpine` / `php-fpm:alpine` / `frankenphp:alpine` conventions. Deployments drop privileges at the orchestrator level (`docker run --user www-data`, Compose `user:`, Kubernetes `runAsUser`). `chown www-data:www-data /var/www/html` still runs at build time.
- SAPI executor split into per-file modules; worker pool hot path tightened.
- Decorator registry migrated from `unsafe static mut` to `OnceLock`.
- Legacy plugin modules removed in favor of `ox_*` rewrites.
- PHP worker config parsing centralized in `Config`.
- Hyper updated to 1.9; unused `serde` dependency dropped.

### Breaking Changes

- **Async namespace migration**: all async-related PHP classes moved under `OxPHP\Async\`:
  - `OxPHP\AsyncException` → `OxPHP\Async\Exception`
  - `OxPHP\AsyncTimeoutException` → `OxPHP\Async\TimeoutException`
  - `OxPHP\AsyncBorrowException` → `OxPHP\Async\BorrowException`
  - `OxPHP\BorrowedProxy` → `OxPHP\Async\BorrowedProxy`
- Async functions (`oxphp_async`, `oxphp_async_await`, etc.) are now provided by the `plugin-async` feature flag. Without it, the functions are not available. Function names are unchanged.
- **Plugin API**: `Plugin::shutdown` now takes `&mut self` (was `&self`). Plugin authors must update implementations.
- **Plugin config**: `env::set_var` side-effects from plugin init no longer propagate to the core server — plugins must publish core-relevant flags through the explicit core-flags API.
- **`RequestComplete` event**: string-serialized metadata replaced with typed fields.

### Performance

- `Shared\Pool` acquire/release uncontested hot path: **≤ 5 µs gate, ~0.9 µs observed in Docker**. Per-thread affinity keeps slots hot in the acquiring thread without cross-thread handoff.
- Map `set` / `get` path avoids serialisation for nested `Shareable` refs — the refcount-bump retain path is cycle-checked before any mutation, so rejected inserts leak nothing.
- Request path: fewer allocations and clones across routing, response assembly and hot-path dispatch.

### Fixed

- Pool chaos reclaim: in-flight slot counts are refunded when a SAPI worker thread panics mid-acquire, so a crashing worker no longer silently burns budget in the surviving workers' view.
- Cross-thread `Shared\*` access no longer depends on the `worker_liveness` hook for Map / Counter / Flag / Once / Mutex — only Pool uses thread-registration (for its affinity + reclaim paths).
- Async worker SIGBUS from cross-thread `MAP_PTR` access.
- `headers_list()` returning empty — the header handler now returns `SAPI_HEADER_ADD`.
- `payload()` returning null for JSON body after a PDO query reused the request buffer.
- `SecurityHeadersHandler` env-variable race: `FRAME_OPTIONS` is now resolved at startup rather than read per request.
- Decorator `RejectedException` dispatch and instance-cache collisions across requests.
- `-Wint-to-pointer-cast` warning in bridge `server_context` assignment.
- Default-feature (`php`) compile/clippy errors that were previously masked by the `--no-default-features` CI profile.
- TLS test profile now generates v3 certificates dynamically and the runner supports HTTPS.
- E2E runner `curl_args` parsing no longer strips shell quotes incorrectly.
- Cancelled-task exception class corrected to `OxPHP\Async\Exception`.

## [0.2.0] - 2026-03-27

### Added

#### Async & Concurrency

- Fiber-based request multiplexing in worker mode — concurrent I/O within a single worker thread
- Async promises (`oxphp_async()`) — parallel PHP execution via dedicated thread pool
- Distributed tracing with W3C Trace Context propagation and OpenTelemetry export

#### PHP API

- HTTP Object API (`OxPHP\Http\Request`) with lazy bridge accessors
- HTTP interfaces (`OxPHP\Http\RequestInterface`, `OxPHP\Http\SessionInterface`, `OxPHP\Http\AttributesInterface`) with clone/serialize blocking on request-scoped classes
- Attribute-based decorator system (`oxphp_register_decorator()`, `OxPHP\Decorator\AttributeInterface`) with PHP observer integration

#### Server Variables

- `HTTPS` — set to `"on"` when TLS is active
- `REQUEST_SCHEME` — `"https"` or `"http"` per PHP-FPM/nginx convention
- `DOCUMENT_URI` — alias for `SCRIPT_NAME` for nginx/PHP-FPM compatibility
- `REQUEST_TIME_FLOAT` — request start time with microsecond precision

#### HTTP Compliance

- `Date` header on all HTTP responses per RFC 9110 §6.6.1
- `Content-Type` header on all error responses per RFC 9110

#### Observability

- Request duration histograms, byte counters, and subsystem metrics
- `trace_context` field exposed in `/config` endpoint

#### Static Files

- `STATIC_CACHE=off` mode with mtime-based content cache revalidation via `stat()` checks

### Changed

- Default listen port changed from 8080 to 80 (TLS-aware: defaults to 443 when `TLS_CERT` is set)
- Backpressure response changed from 503 to 529 (Site is overloaded)
- `workers_idle` metric now calculated dynamically during scrape (was always 0 in static pool mode)
- `workers_spawned_total` counter now includes initial worker spawn
- `/config` endpoint now exposes `log_level` and other missing runtime settings

### Fixed

- `SERVER_PROTOCOL` now reflects actual HTTP version (was hardcoded to `HTTP/1.1`) per RFC 3875
- `REQUEST_TIME` now returns request start time (was returning current time)
- IPv6 Host header parsing for `SERVER_NAME` and `SERVER_PORT`
- Request timeout now returns 408 instead of 504 per RFC 9110
- Duplicate `oxphp_response_time_us_total` Prometheus metric removed
- Session state cleanup added to worker soft reset to prevent state leaks between requests
- Missing fiber source files added to alpine-release Dockerfile

## [0.1.0] - 2026-03-08

First public release. OxPHP replaces nginx + PHP-FPM with a single async binary
written in Rust, providing HTTP serving, native PHP execution via custom SAPI,
and built-in observability.

### Core

- Async HTTP/1.1 server built on Hyper + Tokio with graceful shutdown
- Custom PHP SAPI (`oxphp`) with full superglobals (`$_GET`, `$_POST`, `$_SERVER`, `$_COOKIE`, `$_FILES`)
- C bridge library (`liboxphp_bridge.so`) for zero-copy Rust↔PHP communication via direct zval access
- PHP ZTS (Zend Thread Safety) multi-threaded worker pool with bounded queue and 529 backpressure
- Three routing modes: Traditional (direct file mapping), Framework (front controller), SPA (fallback to `index.html`)
- Static file serving with in-memory cache, MIME detection, and HTTP caching (ETag, Last-Modified, 304 responses)
- Brotli compression with configurable quality level (0–11) and minimum size threshold
- TLS support via PEM certificate/key
- PHP 8.4 compatibility

### Worker Mode

- Persistent PHP worker processes (`oxphp_worker()`) that handle multiple requests with soft reset between them
- Early response completion (`oxphp_finish_request()`) for background processing after response is sent
- SSE streaming with real-time chunked delivery (`oxphp_is_streaming()`, `oxphp_stream_flush()`)
- Cooperative timeout and cancellation via `oxphp_request_heartbeat()`
- Worker mode detection (`oxphp_is_worker()`) and introspection (`oxphp_worker_id()`, `oxphp_server_info()`)
- Resilient to `exit`/`die` in PHP 8.4 worker mode

### Worker Pool

- Static pool mode: fixed number of workers (`PHP_WORKERS=N`)
- Dynamic pool mode: auto-scaling between min and max workers (`PHP_WORKERS=MIN:MAX`)
- Auto-detect mode: defaults to CPU/2 workers (`PHP_WORKERS=0`)
- Per-worker memory limits (`WORKER_MAX_MEMORY_MIB`) and request limits (`WORKER_MAX_REQUESTS`)
- Dead worker respawning via health monitor (static) / scale manager (dynamic)
- `catch_unwind` prevents panics from poisoning the channel

### Observability

- Prometheus metrics endpoint (`/metrics`) with request counts, durations, status codes, active connections, queue depth, and worker mode stats
- Health check endpoint (`/health`)
- Runtime configuration endpoint (`/config`)
- Structured JSON access logging with configurable levels (`ACCESS_LOG`: off/error/all)
- Request ID generation (`oxphp_request_id()`) in `{timestamp:08x}{counter:08x}` format
- Structured PHP error logging via `zend_error_cb`

### Security & Limits

- Per-IP rate limiting (`RATE_LIMIT`, `RATE_WINDOW_SECONDS`)
- Header read timeout (`HEADER_TIMEOUT_SECONDS`) and request timeout (`REQUEST_TIMEOUT_SECONDS`)
- Graceful shutdown with drain timeout (`DRAIN_TIMEOUT_SECONDS`)
- Configurable request body limits
- Path traversal protection with canonicalization

### Plugin System

- Plugin trait with lifecycle hooks (init, startup, shutdown)
- Typed event dispatcher with priority ordering at every lifecycle point
- Events: ConnectionAccepted, RequestReceived, RouteResolved, ScriptExecutionComplete, ResponseBuilding, RequestComplete
- Native PHP function registration from plugins (zero-copy zval access, no JSON serialization)
- Plugin context API for handler registration and configuration

### Performance

- mimalloc global allocator for reduced per-alloc latency
- Configurable multi-threaded Tokio runtime (`TOKIO_WORKERS`)
- Route LRU cache for fast path resolution
- Thread-local buffer reuse for server variables
- Single Arc clone per request (reduced from 10)
- OPcache support with correct `sapi_get_request_time()` initialization

### Infrastructure

- Multi-stage Alpine Docker build (`php:8.4-zts-alpine`)
- Multi-platform Docker images (amd64/arm64) published to GHCR
- CI workflows: nightly build, PR checks (fmt, clippy, tests), release tagging
- Best-practice Dockerfile example with separate dev/prod targets
- HTTP QUERY method support (RFC 9110)
- Documentation in English, Russian, Belarusian, and Chinese

### PHP Functions

| Function | Description |
|---|---|
| `oxphp_request_id()` | Current request identifier |
| `oxphp_worker_id()` | Current worker thread ID |
| `oxphp_server_info()` | Server runtime information |
| `oxphp_request_heartbeat()` | Signal liveness for cooperative timeout |
| `oxphp_finish_request()` | Flush response early, continue background work |
| `oxphp_is_worker()` | Whether running in worker mode |
| `oxphp_is_streaming()` | Whether SSE streaming is active |
| `oxphp_stream_flush()` | Flush SSE chunk to client |
| `oxphp_worker(callable)` | Enter persistent worker loop |

### Environment Variables

| Variable | Default | Description |
|---|---|---|
| `LISTEN_ADDR` | `0.0.0.0:8080` | Server listen address |
| `DOCUMENT_ROOT` | `www/public` | Document root path |
| `INDEX_FILE` | `index.php` | Front controller file |
| `PHP_WORKERS` | CPU/2 | Worker pool size (`N`, `MIN:MAX`, or `0`) |
| `PHP_WORKERS_IDLE_SECONDS` | `30` | Dynamic pool idle timeout |
| `TOKIO_WORKERS` | CPU/2 | Tokio runtime threads (0 = auto) |
| `QUEUE_CAPACITY` | workers×128 | Bounded queue size |
| `RATE_LIMIT` | `0` (off) | Max requests per window per IP |
| `RATE_WINDOW_SECONDS` | `60` | Rate limit window |
| `HEADER_TIMEOUT_SECONDS` | `10` | Header read timeout |
| `REQUEST_TIMEOUT_SECONDS` | `30` | Request execution timeout |
| `DRAIN_TIMEOUT_SECONDS` | `30` | Graceful shutdown drain |
| `COMPRESSION_LEVEL` | `4` | Brotli quality 0–11 (0 = off) |
| `STATIC_CACHE_TTL` | `3600` | Static file Cache-Control max-age |
| `ACCESS_LOG` | `all` | Log level: off/error/all |
| `ERROR_PAGES_DIR` | — | Directory with `{status}.html` pages |
| `TLS_CERT` / `TLS_KEY` | — | TLS certificate/key PEM paths |
| `INTERNAL_ADDR` | `0.0.0.0:9090` | Health/metrics/config endpoint |
| `WORKER_MAX_REQUESTS` | `0` (unlimited) | Max requests per worker before restart |
| `WORKER_MAX_MEMORY_MIB` | `0` (unlimited) | Max worker memory before restart |
| `EXECUTOR` | `sapi` | Executor type: sapi/stub |

[0.5.0]: https://github.com/oxphp/oxphp/releases/tag/v0.5.0
[0.4.0]: https://github.com/oxphp/oxphp/releases/tag/v0.4.0
[0.3.0]: https://github.com/oxphp/oxphp/releases/tag/v0.3.0
[0.2.0]: https://github.com/oxphp/oxphp/releases/tag/v0.2.0
[0.1.0]: https://github.com/oxphp/oxphp/releases/tag/v0.1.0
