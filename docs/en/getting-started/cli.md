---
title: Command-Line Interface
description: The oxphp binary command grammar — serve the HTTP server, run a single PHP script under CLI semantics, validate configuration, and drop privileges with --user.
---

# Command-Line Interface

The `oxphp` binary has three roles: **serve** an HTTP server, **run** a single PHP script to completion, and **config** utilities. Bare `oxphp` with no arguments is an implicit `serve`, so the published image's `CMD ["oxphp"]` keeps starting the server unchanged.

```
USAGE:
    oxphp [OPTIONS]
    oxphp <COMMAND> [OPTIONS]
    oxphp serve [--user=<name|uid[:gid]>]
    oxphp run [-d key=value]... [--user=<spec>] <script.php> [args]...
    oxphp [-d key=value]... [--user=<spec>] <script.php> [args]...

OPTIONS:
    -h, --help      Print this help and exit
    -v, --version   Print version information and exit

COMMANDS:
    serve           Start the HTTP server (default; same as bare 'oxphp')
    run             Execute a single PHP script under CLI semantics and exit
    config          Configuration utilities (see 'oxphp config --help')
```

The role is chosen by keyword. The exact tokens `serve`, `run`, and `config` select a subcommand; **any other first positional argument is treated as a script path** — so `oxphp ./bin/migrate.php` is shorthand for `oxphp run ./bin/migrate.php`. There is no extension heuristic: PHP executes by file content, so an extensionless script runs too. A missing file is reported by the filesystem, exactly like `php`.

## `oxphp serve`

Starts the HTTP application server — the default role. `oxphp` and `oxphp serve` are equivalent. Configuration is via [environment variables](../operations/configuration.md); `serve` itself takes only `--user`.

```bash
oxphp                 # implicit serve
oxphp serve           # explicit
oxphp serve --user=www-data
```

`--user` binds the listeners as the starting user (root, so it can bind ports below 1024) and then permanently drops to the target user before any traffic is served. See [Run as non-root on port 80](docker.md#run-as-non-root-on-port-80-serve---user) for the full pattern, the `<spec>` grammar, and the file-permission checklist.

## `oxphp run`

`oxphp run <script.php> [args…]` executes a single PHP file to completion and exits with the script's own exit code. It runs on the main thread — there is no listener, no worker pool, and no request queue. `PHP_SAPI === 'cli'`, and `phpinfo()` prints text rather than HTML, matching the stock `php` CLI.

```bash
oxphp run migrate.php
oxphp run bin/console.php cache:clear
oxphp run -d memory_limit=512M import.php data.csv
```

This is the role to use for migrations, cron jobs, queue consumers, and `artisan`/`console`-style commands — in the **same image** that serves your application, with no second PHP installation.

The full OxPHP engine is available underneath the script: [fibers](../features/fiber-multiplexing.md) (`oxphp_sleep()`) and [shared state](../shared-state/shared-state.md) (`OxPHP\Shared\*`) work out of the box. [Async promises](../features/async-promises.md) (`oxphp_async()`) work when `ASYNC_WORKERS` is greater than `0`; the default invocation starts no async pool and therefore no background runtime.

`$argv` and `$argc` are populated (`$argv[0]` is the script path), and `STDIN` / `STDOUT` / `STDERR` are defined, so Composer- and Symfony-Console-style entry points run unmodified.

**The script path is the separator.** Every token after the script path is handed to PHP verbatim — including `--`-prefixed ones — so for ordinary arguments you do not need an explicit `--`. Flags meant for `oxphp` itself (`-d`, `--user`) must come *before* the script path; anything after it belongs to the script.

```bash
oxphp run console.php migrate --force --pretend
#            ▲ script  └──────────────┬──────────┘
#                        $argv[1..], passed to PHP verbatim (oxphp parses nothing here)
```

So `oxphp run console.php --force` gives the script `--force` in `$argv`, whereas `oxphp run --force console.php` is an error — `--force` precedes the script path, where `oxphp` only accepts `-d` / `--user` / `--help`.

### The `--` end-of-options marker

`oxphp` honors the standard `--` end-of-options marker, and it matters in exactly one place: **before the script path**, when the script path itself begins with a dash. `--` stops option parsing so the next token is taken as the script path, and `oxphp` consumes it — it is not forwarded to PHP.

```bash
oxphp run -- -odd-name.php      # runs the script "-odd-name.php"
oxphp -- -odd-name.php          # same, implicit form
oxphp run -odd-name.php         # error: parsed as options → "unexpected argument to 'run': -o"
```

After the script path there are no options left to terminate, so a `--` there is ordinary data and is passed to PHP literally, exactly like `php`:

```bash
oxphp run app.php -- --raw      # $argv = ["app.php", "--", "--raw"]
```

### Shebang scripts

A leading `#!` line is skipped before compilation, so an executable, extensionless script with an `oxphp` shebang runs directly:

```php
#!/usr/bin/env oxphp
<?php
echo "hello from a shebang script\n";
```

```bash
chmod +x ./greet
./greet
```

### `-d` ini overrides

`-d key[=value]` sets a `php.ini` directive for this run. It is repeatable, and a bare `-d key` sets the value to `"1"`. These overrides are applied before module startup, so they win over `php.ini` for **every** directive type — including `PHP_INI_SYSTEM` / `PHP_INI_PERDIR` directives (`opcache.*`, `register_argc_argv`, …) that a runtime `ini_set()` cannot change.

```bash
oxphp run -d memory_limit=1G -d display_errors=1 report.php
```

### Default ini for `run`

The `run` role applies CLI-oriented defaults before your `-d` overrides and `php.ini`:

| Directive | Default | Why |
|---|---|---|
| `max_execution_time` | `0` | A one-shot job (migration, importer, daemon) must not be killed by `SIGALRM`. |
| `max_input_time` | `-1` | No input-parsing deadline for CLI. |
| `display_errors` | `stderr` | Errors go to standard error, not standard output. |
| `html_errors` | `0` | Plain-text errors for a terminal. |
| `output_buffering` | `0` | Output is written as it is produced. |
| `implicit_flush` | `1` | Each write is flushed immediately. |
| `register_argc_argv` | `1` | `$argv` / `$argc` are available. |

[Superglobals](../php/superglobals.md) are always enabled for `run` — a one-shot script needs `$argv`, `$_SERVER`, and `$_ENV` — regardless of the `SUPERGLOBALS_ENABLED` toggle that applies to the HTTP server.

### Exit codes

| Code | Meaning |
|---|---|
| script's own | `exit($code)` / `die($code)`, or `0` on a clean finish. |
| `255` | Fatal error, uncaught exception, or parse error. |
| `1` | The script path could not be opened (`oxphp: Could not open input file: <path>`). |
| `2` | An invalid `-d` argument. |

The unopenable-file case is checked before engine startup, so a missing or unreadable script fails fast with the `php`-style message rather than spinning up the engine to die with a compile error.

### `--user` on the run path

`oxphp run --user=<spec> <script.php>` drops OS privileges before the script executes, so a one-shot job started as root can run as an unprivileged user. The `<spec>` grammar and drop mechanics are identical to [`serve --user`](docker.md#run-as-non-root-on-port-80-serve---user); starting as non-root with `--user` is a hard error.

## `oxphp config`

Configuration utilities. `--check` validates the environment-variable configuration and reports problems without starting the server.

```bash
oxphp config --check
```

```
config: OK
```

The check covers filesystem sanity only — path existence and file/directory kind (`DOCUMENT_ROOT`, `ENTRY_FILE`, `TLS_CERT`, `TLS_KEY`, `ERROR_PAGES_DIR`, …). PHP runtime, the TLS handshake, and network binding are out of scope. It exits non-zero and prints `config: INVALID` with one line per problem when validation fails, which makes it suitable as a pre-start gate in an entrypoint or CI job.

## `--help` and `--version`

```bash
oxphp --help            # full usage
oxphp config --help     # config subcommand usage
oxphp --version         # version and the feature flags compiled into this binary
```

`oxphp --version` reports the build's enabled features (for example `php, plugin-apm, plugin-async`), which is the quickest way to confirm whether an image was built with APM, async, or other optional plugins.
