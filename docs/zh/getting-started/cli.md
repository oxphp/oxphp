---
title: 命令行接口
description: oxphp 二进制文件的命令语法 —— 启动 HTTP 服务器（serve）、以 CLI 语义运行单个 PHP 脚本（run）、校验配置，以及使用 --user 降权。
---

# 命令行接口

`oxphp` 二进制文件有三种角色：**serve** 启动 HTTP 服务器，**run** 将单个 PHP 脚本执行至结束，以及 **config** 配置工具。不带任何参数的裸 `oxphp` 等价于隐式的 `serve`，因此发布镜像中的 `CMD ["oxphp"]` 仍能照常启动服务器。

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

角色由关键字选择。`serve`、`run` 和 `config` 这几个确切的标记会选中对应的子命令；**任何其他作为第一个位置参数的内容都会被当作脚本路径** —— 因此 `oxphp ./bin/migrate.php` 是 `oxphp run ./bin/migrate.php` 的简写。这里没有扩展名启发式：PHP 是按文件内容执行的，所以无扩展名的脚本同样能运行。文件缺失由文件系统报告，与 `php` 完全一致。

## `oxphp serve`

启动 HTTP 应用服务器 —— 默认角色。`oxphp` 与 `oxphp serve` 等价。配置通过[环境变量](../operations/configuration.md)完成；`serve` 本身只接受 `--user`。

```bash
oxphp                 # implicit serve
oxphp serve           # explicit
oxphp serve --user=www-data
```

`--user` 会以启动用户（root，因此能绑定 1024 以下的端口）绑定监听器，然后在处理任何流量之前永久降权到目标用户。完整模式、`<spec>` 语法以及文件权限清单，请参阅[在 80 端口以非 root 运行](docker.md#在-80-端口以非-root-运行serve---user)。

## `oxphp run`

`oxphp run <script.php> [args…]` 将单个 PHP 文件执行至结束，并以脚本自身的退出码退出。它在主线程上运行 —— 没有监听器、没有 worker 池，也没有请求队列。`PHP_SAPI === 'cli'`，`phpinfo()` 输出纯文本而非 HTML，与原生 `php` CLI 一致。

```bash
oxphp run migrate.php
oxphp run bin/console.php cache:clear
oxphp run -d memory_limit=512M import.php data.csv
```

这是用于迁移、cron 任务、队列消费者以及 `artisan`/`console` 风格命令的角色 —— 就在为应用提供服务的**同一镜像**中，无需第二套 PHP 安装。

完整的 OxPHP 引擎在脚本底层依然可用：[fiber](../features/fiber-multiplexing.md)（`oxphp_sleep()`）和[共享状态](../shared-state/shared-state.md)（`OxPHP\Shared\*`）开箱即用。[异步 Promise](../features/async-promises.md)（`oxphp_async()`）在 `ASYNC_WORKERS` 大于 `0` 时可用；默认调用不会启动任何异步池，因此也不会启动后台运行时。

`$argv` 和 `$argc` 会被填充（`$argv[0]` 是脚本路径），`STDIN` / `STDOUT` / `STDERR` 已定义，因此 Composer 和 Symfony Console 风格的入口点无需修改即可运行。

**脚本路径就是分隔符。** 它之后的每个标记都会原样交给 PHP —— 包括以 `--` 开头的标记 —— 因此你永远无需显式输入 `--`。给 `oxphp` 自身的标志（`-d`、`--user`）必须放在脚本路径*之前*；之后的任何内容都归脚本所有。

```bash
oxphp run console.php migrate --force --pretend
#            ▲ script  └──────────────┬──────────┘
#                        $argv[1..], passed to PHP verbatim (oxphp parses nothing here)
```

因此 `oxphp run console.php --force` 会把 `--force` 放进脚本的 `$argv`，而 `oxphp run --force console.php` 则是一个错误 —— `--force` 出现在脚本路径之前，而在那个位置 `oxphp` 只接受 `-d` / `--user` / `--help`。

### `--` 选项结束标记

`oxphp` 遵循标准的 `--` 选项结束标记，而它恰好只在一处起作用：**脚本路径之前**，当脚本路径本身以短横线开头时。`--` 会停止选项解析，使下一个标记被当作脚本路径，并由 `oxphp` 消费 —— 它不会被转发给 PHP。

```bash
oxphp run -- -odd-name.php      # runs the script "-odd-name.php"
oxphp -- -odd-name.php          # same, implicit form
oxphp run -odd-name.php         # error: parsed as options → "unexpected argument to 'run': -o"
```

脚本路径之后已没有任何选项需要终止，因此那里的 `--` 只是普通数据，会原样传递给 PHP，与 `php` 完全一致：

```bash
oxphp run app.php -- --raw      # $argv = ["app.php", "--", "--raw"]
```

### Shebang 脚本

开头的 `#!` 行会在编译前被跳过，因此一个带有 `oxphp` shebang 的可执行、无扩展名脚本可以直接运行：

```php
#!/usr/bin/env oxphp
<?php
echo "hello from a shebang script\n";
```

```bash
chmod +x ./greet
./greet
```

### `-d` ini 覆盖

`-d key[=value]` 为本次运行设置一个 `php.ini` 指令。它可重复使用，裸 `-d key` 会将值设为 `"1"`。这些覆盖在模块启动之前应用，因此对**每一种**指令类型都能压过 `php.ini` —— 包括运行时 `ini_set()` 无法更改的 `PHP_INI_SYSTEM` / `PHP_INI_PERDIR` 指令（`opcache.*`、`register_argc_argv`、……）。

```bash
oxphp run -d memory_limit=1G -d display_errors=1 report.php
```

### `run` 的默认 ini

`run` 角色会在你的 `-d` 覆盖和 `php.ini` 之前应用面向 CLI 的默认值：

| 指令 | 默认值 | 原因 |
|---|---|---|
| `max_execution_time` | `0` | 一次性任务（迁移、导入、守护进程）不应被 `SIGALRM` 杀死。 |
| `max_input_time` | `-1` | CLI 没有输入解析的截止时限。 |
| `display_errors` | `stderr` | 错误写入标准错误，而非标准输出。 |
| `html_errors` | `0` | 为终端输出纯文本错误。 |
| `output_buffering` | `0` | 输出随产生随写出。 |
| `implicit_flush` | `1` | 每次写入都立即刷新。 |
| `register_argc_argv` | `1` | `$argv` / `$argc` 可用。 |

无论应用于 HTTP 服务器的 `SUPERGLOBALS_ENABLED` 开关如何，[超全局变量](../php/superglobals.md)对 `run` 始终启用 —— 一次性脚本需要 `$argv`、`$_SERVER` 和 `$_ENV`。

### 退出码

| 退出码 | 含义 |
|---|---|
| 脚本自身 | `exit($code)` / `die($code)`，或正常结束时为 `0`。 |
| `255` | 致命错误、未捕获异常或解析错误。 |
| `1` | 脚本路径无法打开（`oxphp: Could not open input file: <path>`）。 |
| `2` | 无效的 `-d` 参数。 |

无法打开文件的情况在引擎启动之前检查，因此缺失或不可读的脚本会以 `php` 风格的消息快速失败，而不是启动引擎后因编译错误而崩溃。

### run 路径上的 `--user`

`oxphp run --user=<spec> <script.php>` 会在脚本执行之前降权操作系统权限，因此以 root 启动的一次性任务可以以非特权用户运行。`<spec>` 语法和降权机制与 [`serve --user`](docker.md#在-80-端口以非-root-运行serve---user) 完全相同；以非 root 启动却使用 `--user` 是硬性错误。

## `oxphp config`

配置工具。`--check` 校验环境变量配置并报告问题，而不启动服务器。

```bash
oxphp config --check
```

```
config: OK
```

该检查仅覆盖文件系统的健全性 —— 路径是否存在以及文件/目录类型（`DOCUMENT_ROOT`、`ENTRY_FILE`、`TLS_CERT`、`TLS_KEY`、`ERROR_PAGES_DIR`、……）。PHP 运行时、TLS 握手和网络绑定不在范围之内。校验失败时它会以非零退出，打印 `config: INVALID` 并每行报告一个问题，因此适合用作 entrypoint 或 CI 任务中的启动前关卡。

## `--help` 和 `--version`

```bash
oxphp --help            # full usage
oxphp config --help     # config subcommand usage
oxphp --version         # version and the feature flags compiled into this binary
```

`oxphp --version` 会报告该构建启用的功能（例如 `php, plugin-apm, plugin-async`），这是确认某个镜像是否构建了 APM、async 或其他可选插件的最快方式。
