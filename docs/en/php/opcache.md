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
- `opcache.enable_cli` does not affect OxPHP — that setting applies only to SAPIs named `cli` and `phpdbg`. OxPHP registers with the SAPI name `cli-server`, so OPcache is controlled solely via `opcache.enable`. The `opcache.enable_cli` setting is useful if you run PHP CLI in the same container (e.g., for migrations or Artisan commands). The official image `ghcr.io/oxphp/oxphp` does not include PHP CLI, so this setting is not configured there.

To enable OPcache, at minimum:

```ini
zend_extension=opcache

[opcache]
opcache.enable=1
```

## Recommended Production Settings

These settings are optimized for production container deployments where PHP files do not change at runtime. Disable timestamp validation and preload compiled files at startup for maximum throughput.

```ini
zend_extension=opcache

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
| `validate_timestamps` | `0` | When `0`, OPcache never checks the filesystem for changes. Restart the container or call `opcache_reset()` to pick up code changes. |
| `revalidate_freq` | `0` | Seconds between filesystem checks. Has no effect when `validate_timestamps=0`. |
| `file_update_protection` | `0` | Seconds after file modification before the file is eligible for caching. Set to `0` to cache immediately at startup. |

## Development Settings

For development, enable timestamp validation so code changes take effect without restarting the container. Disable JIT to get clearer stack traces during debugging.

```ini
zend_extension=opcache

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
  -v ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro \
  ghcr.io/oxphp/oxphp:0.1.0
```

**Dockerfile:**

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY oxphp.ini /usr/local/etc/php/conf.d/oxphp.ini
COPY --chown=www-data:www-data . /var/www/html
```

**Docker Compose:**

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.1.0
    ports:
      - "80:80"
    volumes:
      - ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro
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

## See Also

- [Docker Guide](../getting-started/docker.md) -- container setup and mounting configuration files
- [Configuration Reference](../operations/configuration.md) -- environment variables for OxPHP
- [Worker Mode](../features/worker-mode.md) -- persistent PHP processes that benefit most from OPcache
