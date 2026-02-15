---
title: OPcache Compatibility
description: How OPcache works with OxPHP's custom SAPI
---

OPcache works out of the box with OxPHP. PHP scripts are compiled once and cached in shared memory, reused across all worker threads for the lifetime of the server process.

## How It Works

OxPHP uses a custom SAPI that identifies itself as `cli-server` to PHP. OPcache recognizes this SAPI name and enables acceleration automatically --- no special configuration is needed.

Because OxPHP uses PHP ZTS (Zend Thread Safety), all worker threads share the same OPcache shared memory segment. A script compiled by one worker is immediately available to all other workers. This gives you the compilation-once-execute-many behavior of OPcache with the concurrency of multiple worker threads.

## The Request Time Requirement

OPcache's `file_update_protection` feature prevents caching files that were modified very recently (within 2 seconds by default). During each request's initialization, OPcache compares the file's modification time against the current request time.

OxPHP's SAPI provides a `get_request_time` callback that returns the current Unix timestamp. This callback is called by PHP during `php_request_startup()`, which means the request time **must** be available before that point.

### What Happens Without Request Time

If the request time returns `0` (the zero epoch), OPcache's file protection check compares every file's `mtime` against January 1, 1970. Since all files were modified after that date, OPcache considers them "too recent" and refuses to cache them. The result is a **0% cache hit rate** --- every request recompiles every script.

OxPHP avoids this by implementing the `get_request_time` SAPI callback to return `SystemTime::now()` as a Unix timestamp with microsecond precision.

## Verifying OPcache Status

Create a diagnostic script to confirm OPcache is active:

```php
<?php
// www/opcache_check.php
if (!function_exists('opcache_get_status')) {
    echo "OPcache extension is not loaded\n";
    exit(1);
}

$status = opcache_get_status();

echo "OPcache enabled: " . ($status['opcache_enabled'] ? 'yes' : 'no') . "\n";
echo "Cached scripts:  " . $status['opcache_statistics']['num_cached_scripts'] . "\n";
echo "Cache hits:      " . $status['opcache_statistics']['hits'] . "\n";
echo "Cache misses:    " . $status['opcache_statistics']['misses'] . "\n";
echo "Hit rate:        " . round($status['opcache_statistics']['opcache_hit_rate'], 1) . "%\n";
echo "Memory used:     " . round($status['memory_usage']['used_memory'] / 1048576, 1) . " MB\n";
echo "Memory free:     " . round($status['memory_usage']['free_memory'] / 1048576, 1) . " MB\n";
```

Test it:

```bash
curl http://localhost:8080/opcache_check.php
# First request: miss (script is compiled and cached)
curl http://localhost:8080/opcache_check.php
# Second request: hit (script served from cache)
```

A healthy OPcache installation shows a hit rate climbing toward 100% after the initial warmup period.

## JIT Compilation

OPcache's JIT compiler is supported when running PHP 8.0+. Enable it in your `php.ini`:

```ini
opcache.enable=1
opcache.jit=1255
opcache.jit_buffer_size=64M
```

JIT provides the most benefit for CPU-intensive PHP code (math, loops, string processing). For I/O-bound applications (database queries, API calls), the improvement is minimal.

## Recommended Settings

These `php.ini` settings are a good starting point for production use with OxPHP:

```ini
[opcache]
opcache.enable=1
opcache.memory_consumption=128
opcache.max_accelerated_files=10000
opcache.validate_timestamps=1
opcache.revalidate_freq=2
opcache.file_update_protection=2
opcache.save_comments=1
```

| Setting | Description |
|---------|-------------|
| `memory_consumption` | Shared memory in MB for compiled scripts. Increase if `opcache_get_status()` shows low free memory. |
| `max_accelerated_files` | Maximum number of cached scripts. Set higher than your total `.php` file count. |
| `validate_timestamps` | When `1`, OPcache checks file modification times on disk. Set to `0` in production if you deploy by replacing the container image. |
| `revalidate_freq` | Seconds between file modification checks. Only applies when `validate_timestamps=1`. |
| `file_update_protection` | Seconds after file modification before caching. Prevents caching partially-written files during deployment. |
| `save_comments` | Keep doc comments in cached scripts. Required by frameworks that use annotation-based routing (e.g., Symfony, Laravel). |

### Production Optimization

For container deployments where PHP files never change at runtime, disable timestamp validation for maximum performance:

```ini
opcache.validate_timestamps=0
```

This tells OPcache to never check the filesystem for changes after a script is cached. You must restart the container (or call `opcache_reset()`) to pick up code changes.

## ZTS and Shared Memory

OxPHP runs PHP in ZTS mode, where each worker thread has its own execution context but all threads share the same OPcache shared memory segment. This means:

- A script compiled by worker 0 is immediately available to workers 1, 2, 3, etc.
- OPcache's internal locking handles concurrent compilation safely.
- Memory consumption does not scale with the number of workers --- one copy of each compiled script serves all threads.

This is more memory-efficient than PHP-FPM, where each process maintains its own OPcache segment (unless you use `opcache.file_cache` with `file_cache_only=1` for shared storage).

## See Also

- [PHP Extension Functions](functions.md) --- the `oxphp_server_info()` function exposes `request_time`
- [Superglobals](superglobals.md) --- how `$_SERVER` and other globals are populated before OPcache's RINIT
- [Worker Pool](/architecture/worker-pool.md) --- ZTS worker threads and shared memory architecture
- [SAPI Bridge](/architecture/sapi-bridge.md) --- the C bridge that provides the `get_request_time` callback
