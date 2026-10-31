---
title: Command-Line Interface
description: The oxphp binary command grammar — serve the HTTP server, run a single PHP script under CLI semantics, validate configuration, and drop privileges with --user.
---

# Command-Line Interface

The `oxphp` binary has three roles: **serve** an HTTP server, **run** a single PHP script to completion, and **config** utilities. Bare `oxphp` with no arguments is an implicit `serve` (one that an `oxphp run` started can read its script from standard input instead, [see below](#oxphp-run)), so the published image's `CMD ["oxphp"]` keeps starting the server unchanged.

```
USAGE:
    oxphp [OPTIONS]
    oxphp <COMMAND> [OPTIONS]
    oxphp serve [--user=<name|uid[:gid]>]
    oxphp run [RUN OPTIONS] <script.php> [args]...
    oxphp [RUN OPTIONS] <script.php> [args]...
    oxphp [RUN OPTIONS] -f <script.php> [args]...
    oxphp [RUN OPTIONS] -r <code> [args]...

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

Starts the HTTP application server — the default role. `oxphp` and `oxphp serve` are equivalent, except in a process that an `oxphp run` started, where a bare `oxphp` can read its script from standard input ([see below](#oxphp-run)). Configuration is via [environment variables](../operations/configuration.md); `serve` itself takes only `--user`.

```bash
oxphp                 # implicit serve — best-effort drop to www-data
oxphp serve           # explicit — same default
oxphp serve --user=appuser   # drop to a specific user (fail-fast)
oxphp serve --user=root      # opt out: keep running as root
```

**By default — with no `--user` — `serve` performs a best-effort drop to `www-data`.** Started as root with a `www-data` account present (as in the official image), it binds the listeners as root and then permanently drops to `www-data` before any traffic is served. Started as non-root it keeps the current user; started as root on a host without a `www-data` account it logs a warning and continues as root. The default never aborts startup.

`--user=<spec>` overrides the target and is **fail-fast**: it binds as root, then permanently drops to that user, and exits with an error if not started as root. Pass `--user=root` to deliberately keep root. See [Run as non-root on port 80](docker.md#run-as-non-root-on-port-80-serve---user) for the full pattern, the `<spec>` grammar, and the file-permission checklist.

## `oxphp run`

`oxphp run <script.php> [args…]` executes a single PHP file to completion and exits with the script's own exit code. It runs on the main thread — there is no listener, no worker pool, and no request queue. `PHP_SAPI === 'cli'`, and `phpinfo()` prints text rather than HTML, matching the stock `php` CLI.

```bash
oxphp run migrate.php
oxphp run bin/console.php cache:clear
oxphp run -d memory_limit=512M import.php data.csv
```

This is the role to use for migrations, cron jobs, queue consumers, and `artisan`/`console`-style commands — in the **same image** that serves your application, with no second PHP installation.

The full OxPHP engine is available underneath the script: [fibers](../features/fiber-multiplexing.md) (`oxphp_sleep()`) and [shared state](../shared-state/shared-state.md) (`OxPHP\Shared\*`) work out of the box, except under [`-n`](#php-options). [Async promises](../features/async-promises.md) (`oxphp_async()`) work when `ASYNC_WORKERS` is greater than `0`; the default invocation starts no async pool and therefore no background runtime. A task that leaves behind a closure it created, for example one stored in `$GLOBALS`, set as an error handler, or held by a `Shared\Pool` the task registers, can crash the process when the task's code is not in OPcache's shared memory, and it can do so while the script is still running, losing the rest of its output and work. Under `run` that is every task unless `opcache.enable_cli=1` is set, because OPcache does not run for the `cli` SAPI by default.

`$argv` and `$argc` are populated (`$argv[0]` is the script path, or `Standard input code` for code given with `-r` or on standard input, as under `php`), and `STDIN` / `STDOUT` / `STDERR` are defined, so Composer- and Symfony-Console-style entry points run unmodified.

As under `php`, the working directory is the one `oxphp` was started in, not the script's directory. The script is opened by the path you give, from that directory: it is not looked up in `include_path`, and `open_basedir` applies to the files it includes and opens, not to the script itself. A phar archive is the exception: it is opened again by its path to read the archive (by `Phar::mapPhar()` in its stub, for example), and `open_basedir` applies to that open, so an archive outside `open_basedir` fails as it does under `php`.

`PHP_BINARY` is the path of the `oxphp` binary (`/usr/local/bin/oxphp` in the published images), as it is the path of `php` under the `php` CLI. A library that starts PHP worker processes through `PHP_BINARY`, such as amphp/parallel, therefore starts `oxphp` for them. Each child is a run of its own: it reads `php.ini` and applies the `run` defaults below by itself, and does not inherit the parent's `-d` overrides. As under `php`, the binary's directory is searched for `php-cli.ini` / `php.ini` before `/usr/local/etc/php`.

Such a child is started with a `php` command line, and `oxphp` takes the [`php` options](#php-options) these libraries pass: Symfony's `PhpSubprocess` passes `-n -c <file>`, and PHPUnit passes `-d`, and `-f` for a `.phpt` test with a `--STDIN--` section. Symfony's `PhpProcess` and PHPUnit's process isolation start the bare binary and write the script to its standard input, and a bare `oxphp` reads it from there, as a bare `php` does, when an `oxphp run` started it. Each run sets the environment variable `OXPHP_RUN_PID` to its own process id, which its children inherit. A bare `oxphp` reads standard input only when its parent's process id is that value and its standard input is not a character device, such as a terminal or `/dev/null`; otherwise it is the server. So a child given an environment of its own that leaves the variable out is the server, and so is a bare `oxphp` started from a shell, as the image's `CMD`, or in the background through a shell, which gives it `/dev/null`. The check can still pass for an `oxphp` the run did not start itself: `system('oxphp')` when `/bin/sh` replaces itself with `oxphp`, or an orphan that a run running as PID 1 adopts. A script under `run` that starts the server should therefore call `oxphp serve`, not a bare `oxphp`. `-v` prints the OxPHP version rather than PHP's. Code on standard input takes no arguments: `oxphp -- <args>` names a script file to run, so a PHPUnit `.phpt` test with an `--ARGS--` section and no `--STDIN--` section fails under `run`. A child is also started without `--user`, so under `oxphp --user=root run` it drops to `www-data` while its parent stays root; an amphp/parallel parent then hangs, waiting for a child that cannot connect back to it.

**The script path is the separator.** Every token after the script path is handed to PHP verbatim — including `--`-prefixed ones — so for ordinary arguments you do not need an explicit `--`. Flags meant for `oxphp` itself (`-d`, `-n`, `-c`, `--user`) must come *before* the script path; anything after it belongs to the script. `-f <file>` and `-r <code>` are options, not a script path: after them, as under `php`, options are read up to the first argument or `--` (see [`php` options](#php-options)).

```bash
oxphp run console.php migrate --force --pretend
#            ▲ script  └──────────────┬──────────┘
#                        $argv[1..], passed to PHP verbatim (oxphp parses nothing here)
```

So `oxphp run console.php --force` gives the script `--force` in `$argv`, whereas `oxphp run --force console.php` is an error — `--force` precedes the script path, where `oxphp` only accepts its own options.

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

After `-f <file>` or `-r <code>`, as under `php`, a `--` before the first argument is consumed and the next token is the first argument:

```bash
oxphp -r 'var_dump($argv);' -- -x   # $argv = ["Standard input code", "-x"]
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

### `php` options

`oxphp` takes these `php` CLI options before the script, with the meaning they have under `php`:

| Option | Effect |
|---|---|
| `-n` | No `php.ini` is loaded except the one `-c` points to, and the `conf.d` directory is not scanned, so the extensions loaded there are not loaded either. In the OxPHP images OxPHP's own extension is one of them: as under `php -n`, `oxphp_sleep()`, `oxphp_async()` and `OxPHP\Shared\*` are then missing unless `-d extension=oxphp_sapi.so` is given too. |
| `-c <path>` | `php.ini` is looked for in `<path>` only: the file itself, or `php-cli.ini` / `php.ini` in that directory. `conf.d` is still scanned unless `-n` is given too. |
| `-f <file>` | Run `<file>`. As under `php`, options are read up to the first argument or `--`, after `<file>` too: `oxphp -f t.php -d k=v` sets `k`, and `oxphp -f t.php --verbose` is an error. The first argument and every token after it are the script's arguments, `serve` and `run` included. |
| `-r <code>` | Run `<code>`, written without the `<?php` tag. `$argv[0]` is `Standard input code` and `__FILE__` is `Command line code`; an uncaught exception exits with `255`. What follows `<code>` is read as after `-f <file>`. |

Only one of `-f` and `-r` may be given, once. `-n` and `-c` need a script, as `-d` does: with nothing to run, `oxphp -n` is an error rather than a server. Other `php` options, such as `-l`, `-a` or `-i`, are an error.

### Default ini for `run`

The `run` role applies CLI-oriented defaults before your `-d` overrides and `php.ini`:

| Directive | Default | Why |
|---|---|---|
| `max_execution_time` | `0` | A one-shot job (migration, importer, daemon) must not be killed by PHP's execution timer. |
| `max_input_time` | `-1` | No input-parsing deadline for CLI. |
| `display_errors` | `stderr` | Errors go to standard error, not standard output. |
| `html_errors` | `0` | Plain-text errors for a terminal. |
| `output_buffering` | `0` | Output is written as it is produced. |
| `implicit_flush` | `1` | Each write is flushed immediately. |
| `register_argc_argv` | `1` on PHP 8.4, not set on 8.5 | `$argv` / `$argc` are available. PHP 8.5 provides them without this directive, as its own `php` binary does; turning it on there, with `-d` or in `php.ini`, makes each async worker thread (`ASYNC_WORKERS` greater than `0`) raise a deprecation notice as it starts, which goes to stderr with these defaults unless your `php.ini` hides it, as `php.ini-production` does. |

`run` honours [`SUPERGLOBALS_ENABLED`](../php/request-api.md#superglobals_enabled). On this path its only effect on the superglobals is whether the process environment is folded into `$_SERVER`. With the default (`true`) the environment, secrets included, is copied there; with `false` it is not. Either way `$_SERVER` keeps the script's own keys — `PHP_SELF`, `SCRIPT_NAME`, `SCRIPT_FILENAME`, `PATH_TRANSLATED` and an empty `DOCUMENT_ROOT` — and `$argv` / `$argc` are unaffected. `oxphp_superglobals_enabled()` reports the setting here as it does under `serve`. `$_ENV` does not depend on this setting: PHP fills it from the environment when `variables_order` contains `E`, and leaves it empty otherwise.

### Exit codes

| Code | Meaning |
|---|---|
| script's own | `exit($code)` / `die($code)`, or `0` on a clean finish. |
| `255` | Fatal error, uncaught exception, or parse error. |
| `1` | The script path could not be opened (`oxphp: Could not open input file: <path>`). |
| `2` | An invalid `-d` argument. |
| `137` | PHP's hard memory limit, on PHP 8.4.27 / 8.5.12 and later: an allocation past the hard limit PHP sets above `memory_limit` that PHP cannot refuse ends the process, and stderr has a `Hard memory limit of … bytes exhausted` line — see [Memory limits](docker.md#memory-limits). A container the kernel kills for running out of memory exits with `137` too, without that line. |

The unopenable-file case is checked before engine startup, so a missing or unreadable script fails fast with the `php`-style message rather than spinning up the engine to die with a compile error.

### `--user` on the run path

`run` follows the same default as `serve`: with no `--user` it performs a best-effort drop to `www-data` before the script executes (skipped silently when already non-root). `oxphp run --user=<spec> <script.php>` overrides the target so a one-shot job started as root can run as a specific unprivileged user; `--user=root` keeps root. The `<spec>` grammar and drop mechanics are identical to [`serve --user`](docker.md#run-as-non-root-on-port-80-serve---user); an explicit `--user` started as non-root is a hard error.

## `oxphp config`

Configuration utilities. `--check` validates the environment-variable configuration and reports problems without starting the server.

```bash
oxphp config --check
```

```
config: OK
```

Validation covers filesystem sanity only — path existence and file/directory kind (`DOCUMENT_ROOT`, `ENTRY_FILE`, `TLS_CERT`, `TLS_KEY`, `ERROR_PAGES_DIR`, …). PHP runtime, the TLS handshake, and network binding are out of scope. It exits non-zero and prints `config: INVALID` with one line per problem when validation fails, which makes it suitable as a pre-start gate in an entrypoint or CI job.

A sizing advisory can follow the verdict on a `  ! ` line — currently one, for a PHP queue sized to hold every connection the server is allowed to accept:

```
config: OK
  ! PHP_WORKERS (7) + QUEUE_CAPACITY (896) + QUEUE_MAX_WAITING (500) = 1403 >= MAX_CONNECTIONS (1000): …
```

It is not a validation failure: the configuration is legal and the exit code is unchanged, so the same line can follow either verdict. A gate should therefore test the exit code rather than match the output exactly.

## `--help` and `--version`

```bash
oxphp --help            # full usage
oxphp config --help     # config subcommand usage
oxphp --version         # version and the feature flags compiled into this binary
```

`oxphp --version` reports the build's enabled features (for example `php, plugin-apm, plugin-async`), which is the quickest way to confirm whether an image was built with APM, async, or other optional plugins.
