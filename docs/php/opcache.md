---
title: OPcache and JIT
description: Configure OPcache and JIT compilation for optimal PHP performance with OxPHP, including preloading and development settings.
---

# OPcache and JIT

OPcache works out of the box with OxPHP. All PHP worker threads share a single OPcache memory segment — scripts are compiled once at first execution and served from the cache by every worker thereafter. No special setup is required to enable this sharing.

## How OPcache Works with OxPHP

OxPHP registers itself as a named SAPI, and OPcache treats it identically to other server SAPIs. The key characteristics are:

- **Shared cache across workers**: all PHP worker threads use the same compiled opcode cache. One worker compiles a file; all workers benefit.
- **No per-request compilation**: after the first request for each script, subsequent requests skip the parse and compile step entirely.
- `opcache.enable_cli` does not affect OxPHP — that setting applies only to SAPIs named `cli` and `phpdbg`. OxPHP registers with the SAPI name `cli-server`, so OPcache is controlled solely via `opcache.enable`. The `opcache.enable_cli` setting is useful if you run PHP CLI in the same container (e.g., for migrations or Artisan commands). The official OxPHP image ships PHP CLI alongside the server binary, so you can set `opcache.enable_cli=1` if your CLI scripts benefit from caching.

To enable OPcache, at minimum:

```ini
[opcache]
opcache.enable=1
```

> **Note:** The official OxPHP Docker image is based on `php:*-zts-alpine`, which compiles OPcache statically into the PHP binary. Do NOT add `zend_extension=opcache` to your INI file — the extension is already loaded, and adding that line will cause a warning on every PHP startup. Only the `[opcache]` configuration section is needed.

## Recommended Production Settings

These settings are optimized for production container deployments where PHP files do not change at runtime. Disable timestamp validation and preload compiled files at startup for maximum throughput.

```ini
[opcache]
opcache.enable=1
opcache.memory_consumption=128
opcache.interned_strings_buffer=16
opcache.max_accelerated_files=10000
opcache.validate_timestamps=0
opcache.revalidate_freq=0
opcache.file_update_protection=0
opcache.jit_buffer_size=64M
opcache.jit=tracing
```

| Setting | Recommended Value | Description |
|---------|-------------------|-------------|
| `memory_consumption` | `128` | Shared memory in MB for compiled scripts. Increase if `opcache_get_status()` shows low free memory. |
| `interned_strings_buffer` | `16` | Memory in MB for interned strings shared across all workers. |
| `max_accelerated_files` | `10000` | Maximum number of cached scripts. Set this higher than your total `.php` file count. |
| `validate_timestamps` | `0` | When `0`, OPcache never checks the filesystem for changes. Restart the container to pick up code changes. `opcache_reset()` picks them up too, at the cost described in [OPcache Restarts](#opcache-restarts). |
| `revalidate_freq` | `0` | Seconds between filesystem checks. Has no effect when `validate_timestamps=0`. |
| `file_update_protection` | `0` | Seconds after file modification before the file is eligible for caching. Set to `0` to cache immediately at startup. |

## Development Settings

For development, enable timestamp validation so code changes take effect without restarting the container. Disable JIT to get clearer stack traces during debugging.

```ini
[opcache]
opcache.enable=1
opcache.memory_consumption=128
opcache.interned_strings_buffer=16
opcache.max_accelerated_files=10000
opcache.validate_timestamps=1
opcache.revalidate_freq=2
opcache.jit_buffer_size=0
opcache.jit=disable
```

With `validate_timestamps=1`, OPcache checks file modification times every `revalidate_freq` seconds. This adds a small per-request overhead but lets you edit PHP files and see changes on the next request.

**This is the recommended development-mode reload strategy for OxPHP.** OPcache performs the check inline on every include, so code edits are picked up at the next request with no container restart and no external file-watcher daemon. Use `revalidate_freq=0` for immediate stat-per-include (highest accuracy, slightly more I/O), or `revalidate_freq=2` to amortize the stat cost — the default shown above is a good balance, especially if your `DOCUMENT_ROOT` is on a slow bind-mount (Docker on macOS/Windows).

### What is NOT reloaded by `validate_timestamps`

A few classes of changes still require a container restart (or worker recycle) even with `validate_timestamps=1`:

- **Preloaded files** (`opcache.preload`) are linked into the server at startup and are never revalidated. Edit a preloaded file — restart the container.
- **Worker Mode bootstrap state** — in [Worker Mode](../features/worker-mode.md), the autoloader, DI container, and any objects built in the outer scope live in worker memory. OPcache will recompile changed class files, but the worker will not re-run its bootstrap. For dev worker loops, call [`Worker::scheduleExit()`](worker-class.md#scheduleexit) at the end of each request (e.g. behind an `OXPHP_DEV` env flag) to recycle the worker, which re-executes the outer scope and picks up all changes.
- **Framework-level caches** — compiled Symfony container, Laravel route/config/view cache, Composer optimized classmap. These are `.php` files OPcache revalidates, but the values inside them reference stale class paths or container IDs. Run the framework's `cache:clear` command; OPcache alone is not enough.
- **Non-PHP files** — `.env`, `composer.json`, YAML/JSON config, template files compiled outside OPcache. OPcache tracks only files it compiled; everything else needs a restart.

## JIT Compilation

OPcache's JIT compiler translates PHP opcodes into native machine code at runtime. Use `tracing` mode for the best optimization:

```ini
opcache.jit=tracing
opcache.jit_buffer_size=64M
```

JIT provides the most benefit for CPU-bound PHP code — math-heavy loops, string processing, image manipulation, and template rendering. For I/O-bound applications that spend most of their time waiting on database queries or external API calls, the improvement is minimal.

To disable JIT:

```ini
opcache.jit=disable
opcache.jit_buffer_size=0
```

## Preloading

OPcache preloading compiles and caches PHP files at server startup, before any requests are handled. This eliminates first-request compilation cost entirely and makes classes and functions available globally without any `require` or autoload overhead.

Configure preloading in your INI file:

```ini
opcache.preload=/var/www/html/preload.php
opcache.preload_user=www-data
```

Create a `preload.php` script that loads your most frequently used files:

```php
<?php
// preload.php — runs once at server startup

require __DIR__ . '/vendor/autoload.php';

// Preload framework core files
$files = glob(__DIR__ . '/vendor/symfony/http-kernel/**.php');
foreach ($files as $file) {
    opcache_compile_file($file);
}

// Preload hot application paths
opcache_compile_file(__DIR__ . '/src/Controller/ApiController.php');
opcache_compile_file(__DIR__ . '/src/Service/UserService.php');
```

> **Note:** Preloaded classes and functions are permanently available to all requests. They cannot be changed without restarting the server.

> **Worker Mode and preloading:** If you use [Worker Mode](../features/worker-mode.md), your application is already initialized once — the autoloader, configuration, and database connections persist between requests. OPcache preloading complements this by eliminating opcode compilation overhead, but it does not replace application initialization. The two mechanisms work independently and can be used together.

## Applying the PHP Configuration

OxPHP reads PHP configuration from the standard `conf.d` directory. Use a Docker volume or a `COPY` instruction to supply your custom INI file.

**Docker run:**

```bash
docker run -p 80:80 \
  -v ./custom.ini:/usr/local/etc/php/conf.d/custom.ini:ro \
  ghcr.io/oxphp/oxphp:0.15.0
```

**Dockerfile:**

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.15.0

COPY custom.ini /usr/local/etc/php/conf.d/custom.ini
COPY --chown=www-data:www-data . /var/www/html
```

**Docker Compose:**

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.15.0
    ports:
      - "80:80"
    volumes:
      - ./custom.ini:/usr/local/etc/php/conf.d/custom.ini:ro
      - ./src:/var/www/html
```

## Monitoring Cache Status

Inspect the live OPcache status from PHP to verify it is working:

```php
<?php
$status = opcache_get_status();

echo "Cached scripts: " . $status['opcache_statistics']['num_cached_scripts'] . "\n";
echo "Cache hits: "     . $status['opcache_statistics']['hits'] . "\n";
echo "Cache misses: "   . $status['opcache_statistics']['misses'] . "\n";
echo "Free memory: "    . $status['memory_usage']['free_memory'] . " bytes\n";
```

If `free_memory` is consistently low, increase `opcache.memory_consumption`.

## OPcache Restarts

OPcache restarts its shared memory — drops every cached script and starts over — when `opcache_reset()` is called, and when its memory (`opcache.memory_consumption`) or its table of scripts (`opcache.max_accelerated_files`) is full while at least `opcache.max_wasted_percentage` of the memory (5 % by default) is taken by scripts it has discarded, such as files that changed after it cached them. A restart must not run while code from that memory is still running. Under PHP-FPM, OPcache waits until no other process is still running a request that has used the cache. All of OxPHP's PHP threads are one process, which OPcache's own check cannot see into, so on Linux OxPHP holds the restart itself:

- Requests that start after OPcache has scheduled the restart run without the cache, as under PHP-FPM: they compile what they include, or load it from `opcache.file_cache` when that is set, and do not wait. Such a request runs without the cache to its end, even once the restart has run, and from then on holds the next restart: with `opcache.file_cache`, a script it includes after the restart that the file cache already holds is loaded from there into the cache's new memory.
- The restart runs in the startup of the first request to start once nothing that can run code from the cache is left. Each of these holds it until it ends: a request that started with the cache, an [`oxphp_async()`](../features/async-promises.md) task whose closure came from the cache, a pool thread that has run such a task, a worker or pool thread that has called the factory or destroy callback of a `Shared\Pool` another thread created (even a call the pool refused), and a request that started while an earlier restart waited. Requests that start after it use the cache again.
- In [Worker Mode](../features/worker-mode.md), a worker keeps the PHP request it opened at boot until it exits, so a worker that booted with the cache holds the restart and is retired at its next moment with no request in flight. A worker that booted while the restart waited runs without the cache, and is retired at such a moment once the restart has run, or once nothing else holds it, so that its replacement runs the restart. Each replacement runs the worker script's bootstrap again: a restart costs up to two rounds of worker replacement, plus one each time a worker that booted while the restart waited calls such a `Shared\Pool` callback before the restart has run. Workers are not retired during a graceful shutdown.
- The threads of the `oxphp_async()` pool are replaced the same way, at a moment they run no task. A thread that started while the restart waited and runs a task whose closure came from the cache before the restart has run holds it, and is replaced again.
- A worker or pool thread that a steady load leaves no such moment stops taking new requests or tasks to reach one, for at most 5 seconds, and does not try again for that restart if what it runs outlasts them. Threads of a kind do this in turn while the others carry the load: the next starts when a new thread of that kind is ready or the wait of the last one has run out. A thread that started while the restart waited and holds it because it ran a task or a `Shared\Pool` callback from the cache does this only after 5 seconds without another, since while they keep coming its replacement would hold the restart again. With `PHP_WORKERS=1` or `ASYNC_WORKERS=1` nothing else takes the work meanwhile, which can come twice per restart and lasts until the replacement has started: requests wait and are refused once `QUEUE_WAIT_TIMEOUT_MS` has passed, and tasks stay on the queue, where a task dispatched while it is full is refused as usual.

A holder that does not end — a Server-Sent Events stream or a long poll that started with the cache, a task in an endless loop whose closure came from the cache — keeps the restart waiting for as long as it runs. So does a request or task on a worker or pool thread that lasts longer than those 5 seconds: the thread goes back to taking work and is replaced at its next moment with nothing in flight, which a steady load may not leave it. With `ASYNC_WORKERS=1` that includes a task waiting for another task, which cannot start while the thread takes none. A steady stream of tasks whose closures `opcache.preload` loaded keeps the restart waiting too: each one holds it from its dispatch, whichever request dispatches it. A stream that started while the restart waited does not hold it, but holds the next one. OxPHP stops none of them. `opcache.force_restart_timeout` (180 seconds by default), after which OPcache kills the processes that still hold a restart under PHP-FPM, only makes the server log a warning once a restart has waited that long, with the number of requests and tasks holding it (`holders`); `0` turns the warning off. Until the restart runs, requests run without the cache.

The log at `info` level records each restart and each thread it replaces:

| Message | When |
|---------|------|
| `OPcache restart scheduled` | The request that scheduled the restart has ended. `reason` is `opcache_reset()`, `out of memory` or `hash overflow`. |
| `Worker retiring: …`, `Async worker thread retiring: …` | A worker or pool thread is replaced: `an OPcache restart is waiting for it`, or `it opened while an OPcache restart was pending`. |
| `Worker taking no new requests …`, `Async worker thread taking no new tasks …` | A worker or pool thread has stopped taking new work to reach a moment with none in flight. |
| `Worker taking requests again: …`, `Async worker thread taking tasks again: …` | It has gone back to taking work, because what it runs outlasted the wait or going without work no longer helps the restart. |
| `OPcache restart done` | The restart has run; `waited_ms` is how long it waited. |
| `OPcache restart still waiting, …` (`warn`) | The restart has waited longer than `opcache.force_restart_timeout`. |

To deploy new code, prefer replacing the container: `opcache_reset()` runs every request without the cache until the restart and, in Worker Mode, boots every worker again.

## See Also

- [Docker Guide](../getting-started/docker.md) -- container setup and mounting configuration files
- [Configuration Reference](../operations/configuration.md) -- environment variables for OxPHP
- [Worker Mode](../features/worker-mode.md) -- persistent PHP workers that benefit most from OPcache
