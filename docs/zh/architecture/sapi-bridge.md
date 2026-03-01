---
title: SAPI 与 Bridge
description: OxPHP 的自定义 PHP SAPI、带 __thread TLS 的 C bridge 库与 PHP 扩展 API
---

OxPHP 使用自定义 SAPI（Server API）与 PHP 集成，而非标准的 `php-embed` SAPI。一个共享的 C bridge 库提供了 Rust 二进制文件与 PHP 扩展之间共享每请求状态的机制。本页解释此架构存在的原因及各组件如何交互。

## 为什么使用自定义 SAPI？

PHP 的 SAPI 层是 Web 服务器与 PHP 引擎之间的接口。标准 SAPI（cli、fpm、embed）对进程生命周期的假设不适合 OxPHP 的模型：

- **php-embed** 期望每进程一个请求，不支持多线程上的并发请求处理。
- **php-fpm** 是独立的进程管理器。OxPHP 消除了进程间通信的需要。
- **php-cli** 没有 HTTP 集成。

OxPHP 注册自己的 `sapi_module_struct`，名称为 `"oxphp"`。这提供了对以下内容的完全控制：

- 输出捕获（拦截 PHP 的输出缓冲区）
- 头处理（收集 `header()` 调用）
- `php://input`（提供请求体）
- `$_SERVER` 填充（从 Rust 侧请求数据设置超全局变量）
- 请求时间（通过 `sapi_get_request_time`）

## Bridge 问题

当 OxPHP 的 Rust 二进制文件编译时，它链接 `libphp.so`。PHP 扩展由 `libphp.so` 在运行时通过 `dlopen()` 加载。这造成了可见性问题：

```
┌────────────────────┐         ┌───────────────────┐
│  Rust Binary       │         │  libphp.so        │
│                    │ links   │                   │
│  thread_local! {   │────────▶│  dlopen() ───────▶│ oxphp_sapi.so
│    // Rust TLS     │         │                   │  (PHP extension)
│  }                 │         └───────────────────┘
└────────────────────┘                             │
                                                   │
  Rust thread_local! vars are INVISIBLE            │
  to dlopen'd shared libraries ──────────────────▶ │
```

Rust 的 `thread_local!` 宏使用 ELF TLS 或平台特定机制，在链接时解析。通过 `dlopen()` 在运行时加载的共享库无法看到这些符号。这意味着 PHP 扩展无法直接读取 Rust 存储在线程本地存储中的请求数据。

## Bridge 库

解决方案是 `liboxphp_bridge.so` — 一个小型 C 共享库，Rust 二进制文件和 PHP 扩展都链接它。它使用 C `__thread` TLS，对共享同一地址空间的所有 `dlopen` 库可见。

```
┌────────────────────┐
│  Rust Binary       │──links──┐
└────────────────────┘         │
                               ▼
                    ┌──────────────────────┐
                    │  liboxphp_bridge.so  │
                    │                      │
                    │  static __thread     │
                    │    oxphp_ctx_t ctx;  │
                    │                      │
                    │  static (global)     │
                    │    plugin_functions  │
                    │    native_dispatch   │
                    └──────────────────────┘
                               ▲
┌────────────────────┐         │
│  oxphp_sapi.so     │──links──┘
│  (PHP extension)   │
└────────────────────┘
```

Rust 二进制文件和 PHP 扩展都调用 `liboxphp_bridge.so` 中的函数来读写同一个 `__thread` 变量。由于它们在同一进程且同一 OS 线程上，共享同一个 TLS 槽。

### Bridge 上下文

每请求上下文定义在 `ext/bridge/oxphp_bridge.h` 中：

```c
typedef struct {
    char request_id[65];    // 十六进制请求 ID（64 字符 + null）
    int32_t worker_id;      // 工作线程索引
    double request_time;    // Unix 时间戳，微秒
    bool stream_mode;       // 流式模式是否激活
    bool headers_sent;      // 头是否已发送（流式）
    bool finished;          // oxphp_finish_request() 是否已调用
} oxphp_ctx_t;
```

### Bridge API

Bridge 暴露操作 `__thread` 本地 `ctx` 变量的 getter/setter 函数：

| 函数 | 用途 |
|---|---|
| `oxphp_bridge_init_ctx()` | 零初始化上下文（在 `php_request_startup` 之前调用） |
| `oxphp_bridge_clear_ctx()` | 在请求关闭后清零上下文 |
| `oxphp_bridge_get_ctx()` | 获取上下文结构体指针 |
| `oxphp_bridge_set_request_id(id)` | 复制请求 ID（最多 64 字符） |
| `oxphp_bridge_get_request_id()` | 获取请求 ID 指针 |
| `oxphp_bridge_set_worker_id(id)` | 设置工作线程索引 |
| `oxphp_bridge_set_request_time(time)` | 设置请求开始时间 |
| `oxphp_bridge_get_request_time()` | 获取请求开始时间 |
| `oxphp_bridge_set_stream_mode(mode)` | 启用/禁用流式模式 |
| `oxphp_bridge_is_streaming()` | 检查流式模式是否激活 |
| `oxphp_bridge_set_finished(bool)` | 标记请求已完成 |
| `oxphp_bridge_is_finished()` | 检查请求是否已完成 |
| `oxphp_bridge_set_headers_sent(bool)` | 标记头已发送 |
| `oxphp_bridge_get_headers_sent()` | 检查头是否已发送 |

实现在 `ext/bridge/oxphp_bridge.c` 中非常直接 — 每个函数读取或写入 `static __thread oxphp_ctx_t ctx` 变量的一个字段。

### 关键不变量

**`init_ctx()` 和 `set_request_time()` 必须在 `php_request_startup()` 之前调用。**

OPcache 的 RINIT 处理器在 `php_request_startup()` 期间读取 `sapi_get_request_time()`。自定义 SAPI 的 `sapi_get_request_time` 回调从 bridge 上下文读取。如果 bridge 返回 0（未初始化），OPcache 的 `file_update_protection` 检查失败，导致 0% 缓存命中率。

每个工作线程上的正确调用顺序：

```
1. oxphp_bridge_init_ctx()
2. oxphp_bridge_set_request_id(...)
3. oxphp_bridge_set_request_time(...)
4. sapi::set_request_data(request)    // server vars, cookies, body
5. php_request_startup()              // 触发所有扩展的 RINIT
6. php_execute_script(...)
7. php_request_shutdown()
8. oxphp_bridge_clear_ctx()
```

## 插件函数注册表

Bridge 还提供一个**全局**（非 `__thread`）插件函数注册表。它允许 Rust 插件注册 PHP 脚本可以调用的函数，以及 Rust 可以调用的 PHP 函数。

### 注册表 API

| 函数 | 用途 |
|---|---|
| `oxphp_bridge_register_plugin_fn(name, required, total)` | 注册插件函数（启动期间由 Rust 调用） |
| `oxphp_bridge_get_plugin_fn_count()` | 获取已注册插件函数数量 |
| `oxphp_bridge_get_plugin_fn_name(index)` | 按索引获取插件函数名 |
| `oxphp_bridge_get_plugin_fn_required(index)` | 按索引获取必需参数数量 |
| `oxphp_bridge_get_plugin_fn_total(index)` | 按索引获取总参数数量 |
| `oxphp_bridge_set_native_dispatch(fn)` | 设置 Rust 原生分发回调 |
| `oxphp_bridge_get_native_dispatch()` | 获取 Rust 原生分发回调 |

注册表是全局的（非每线程），因为它在启动期间只从主线程写入一次，在 MINIT 期间读取 — 无并发访问。它永不释放，存在于整个进程生命周期中。

### Native Bridge：零序列化跨边界调用

Rust 和 PHP 通过直接 `zval` 指针访问通信 — 无 JSON 序列化。`liboxphp_bridge.so` 中的 C 访问器函数提供安全、类型检查的接口来读写 PHP 值：

**读取参数（PHP -> Rust）：**

| 函数 | 用途 |
|---|---|
| `oxphp_val_type(zval*)` | 获取 zval 的类型（IS_LONG、IS_DOUBLE、IS_STRING 等） |
| `oxphp_arg_long(zval*)` | 读取长整型参数 |
| `oxphp_arg_double(zval*)` | 读取双精度浮点参数 |
| `oxphp_arg_str(zval*, len*)` | 读取字符串参数（指针 + 长度） |
| `oxphp_arg_bool(zval*)` | 读取布尔参数 |

**写入返回值（Rust -> PHP）：**

| 函数 | 用途 |
|---|---|
| `oxphp_ret_long(zval*, val)` | 写入长整型返回值 |
| `oxphp_ret_double(zval*, val)` | 写入双精度浮点返回值 |
| `oxphp_ret_str(zval*, str, len)` | 写入字符串返回值 |
| `oxphp_ret_bool(zval*, val)` | 写入布尔返回值 |
| `oxphp_ret_null(zval*)` | 写入 null 返回值 |

**原生分发流程：**

`oxphp_bridge_set_native_dispatch(fn)` 注册 Rust 回调。当 PHP 脚本调用插件函数时，扩展中的 `ZEND_FUNCTION(oxphp_native_dispatch)` 调用此回调，直接传递原始 `zval*` 指针作为参数和返回值 — 不发生序列化。

**从 Rust 调用 PHP：**

`oxphp_call_php_native(func_name, args, argc, result)` 允许 Rust 调用 PHP 用户空间函数。C 侧通过 `zend_hash_str_find_ptr` 解析函数并直接调用 `zend_call_known_function`。结果 zval 由 Rust 拥有，在 drop 时通过 `zval_ptr_dtor` 释放。

## PHP 扩展

PHP 扩展（`ext/oxphp_sapi.c`）向 PHP 脚本暴露服务器特定函数。它链接 `liboxphp_bridge.so` 以读取 bridge 上下文。

### 可用函数

| 函数 | 返回类型 | 描述 |
|---|---|---|
| `oxphp_request_id()` | `string` | 返回当前请求的十六进制请求 ID |
| `oxphp_worker_id()` | `int` | 返回工作线程索引（从 0 开始） |
| `oxphp_server_info()` | `array` | 返回 `sapi`、`version`、`worker_id`、`request_time`、`worker_mode` |
| `oxphp_request_heartbeat(int $time = 10)` | `bool` | 超时延长占位符（当前返回 `true`） |
| `oxphp_finish_request()` | `bool` | 标记请求已完成，用于后台处理 |
| `oxphp_is_worker()` | `bool` | 检查服务器是否运行在 worker 模式下 |
| `oxphp_is_streaming()` | `bool` | 检查当前请求是否使用流式模式 |

### 原生插件分发

扩展注册 `oxphp_native_dispatch` — 所有插件注册函数的零序列化处理器。当 PHP 脚本调用插件函数（例如 `oxphp_example_info()`）时，Zend 引擎分发到 `oxphp_native_dispatch`，它：

1. 从 `execute_data->func->common.function_name` 读取函数名
2. 将原始 `zval*` 指针（参数和返回值）直接通过 bridge 回调传递给 Rust — 无序列化
3. Rust 通过 C 访问器函数（`oxphp_arg_long`、`oxphp_ret_str` 等）读写 zval
4. 出错时发出 PHP `E_WARNING` 并返回 `NULL`

### 从 Rust 调用 PHP

Bridge 提供 `oxphp_call_php_native()` — Rust 可以调用此函数来执行 PHP 用户空间函数：

1. Rust 调用 `oxphp_call_php_native(func_name, args, argc, result)`，传入预构建的 zval 参数
2. C 侧通过 `zend_hash_str_find_ptr` 解析函数并直接调用 `zend_call_known_function`
3. 结果 zval 由 Rust 拥有，在 drop 时通过 `zval_ptr_dtor` 释放

### 示例用法

```php
<?php
// 获取服务器分配的请求 ID
$requestId = oxphp_request_id();
header("X-Debug-Worker: " . oxphp_worker_id());

// 查看 SAPI 详情
$info = oxphp_server_info();
// $info = [
//     'sapi' => 'oxphp',
//     'version' => '0.1.0',
//     'worker_id' => 3,
//     'request_time' => 1707609600.123456,
// ]

// 完成响应但继续处理
oxphp_finish_request();
// ... 后台工作（日志记录、清理等）
```

### 扩展注册

扩展作为标准 PHP 模块注册，带有设置插件函数 bridge 的 MINIT 钩子：

```c
zend_module_entry oxphp_sapi_module_entry = {
    STANDARD_MODULE_HEADER,
    "oxphp_sapi",
    oxphp_sapi_functions,
    PHP_MINIT(oxphp_sapi),  // 设置 call_php 回调，注册插件函数
    NULL,                    // MSHUTDOWN
    NULL,                    // RINIT
    NULL,                    // RSHUTDOWN
    PHP_MINFO(oxphp_sapi),
    "0.1.0",
    STANDARD_MODULE_PROPERTIES
};
```

**MINIT** 执行两项任务：

1. 设置 `oxphp_bridge_set_native_dispatch(oxphp_native_dispatch)` 使 bridge 知道插件函数从 PHP 调用时应调用哪个函数
2. 从 bridge 读取插件函数注册表，并通过 `zend_register_functions()` 向 Zend 注册每个函数 — 这必须在模块启动时（而非请求启动时）完成，以便 OPcache 的编译时 `function_exists()` 优化能看到这些函数

## 数据流总结

```
Rust (Tokio task)                     PHP Worker Thread
─────────────────                     ──────────────────
ScriptRequest ──crossbeam_channel::bounded──▶ recv()
                                      │
                                      ├── bridge::init_ctx()
                                      ├── bridge::set_request_id()
                                      ├── bridge::set_request_time()
                                      ├── sapi::set_request_data()
                                      │     ├── server vars → TLS
                                      │     ├── cookies → TLS
                                      │     └── body → TLS
                                      │
                                      ├── php_request_startup()
                                      │     ├── RINIT for all extensions
                                      │     └── OPcache reads request_time
                                      │
                                      ├── php_execute_script()
                                      │     ├── PHP reads $_SERVER, $_GET, etc.
                                      │     ├── PHP calls oxphp_request_id()
                                      │     │     └── bridge::get_request_id()
                                      │     ├── PHP calls plugin function
                                      │     │     └── bridge::dispatch() → Rust
                                      │     └── Output captured by SAPI
                                      │
                                      ├── php_request_shutdown()
                                      │
                                      ├── sapi::take_response()
                                      │     ├── output buffer
                                      │     ├── response headers
                                      │     └── status code
                                      │
                                      └── bridge::clear_ctx()
                                      │
ScriptResponse ◀──oneshot──────────── tx.send()
```

## 构建 Bridge 和扩展

Bridge 库和 PHP 扩展作为 Docker 镜像的一部分构建。本地开发时：

```bash
# 构建 bridge 库
cd ext/bridge
make
sudo make install  # 安装 liboxphp_bridge.so

# 构建 PHP 扩展
cd ext
phpize
./configure --enable-oxphp-sapi
make
sudo make install  # 安装 oxphp_sapi.so
```

两个产物在运行时都必须可用：
- `liboxphp_bridge.so` 在库搜索路径中（`LD_LIBRARY_PATH=/usr/local/lib`）
- `oxphp_sapi.so` 在 PHP 扩展目录中（或通过 `php.ini` 中的 `extension=oxphp_sapi.so` 加载）

## 另请参阅

- [架构概览](./overview.md) — 组件全景与启动序列
- [工作线程池](./worker-pool.md) — 调用 bridge 的工作线程生命周期
- [请求生命周期](./request-lifecycle.md) — 从 TCP 到响应的完整请求管道
- [PHP 函数](../php/functions.md) — PHP 可调用函数参考
- [超全局变量](../php/superglobals.md) — `$_SERVER`、`$_GET` 等如何填充
- [OPcache](../php/opcache.md) — OPcache 集成与 `request_time` 不变量
