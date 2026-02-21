---
title: OPcache 兼容性
description: OPcache 如何与 OxPHP 的自定义 SAPI 协同工作
---

OPcache 与 OxPHP 开箱即用。PHP 脚本编译一次并缓存在共享内存中，在服务器进程的整个生命周期内被所有工作线程复用。

## 工作原理

OxPHP 使用自定义 SAPI，向 PHP 标识为 `cli-server`。OPcache 识别此 SAPI 名称，但默认不会为 cli-server SAPI 激活。附带的 `oxphp.ini` 包含 `opcache.enable_cli = 1`，这正是为此 SAPI 启用 OPcache 的设置。没有该设置，无论其他配置如何，OPcache 都不会激活。

由于 OxPHP 使用 PHP ZTS（Zend 线程安全），所有工作线程共享同一个 OPcache 共享内存段。一个工作线程编译的脚本立即可供所有其他工作线程使用。这在多工作线程并发的同时实现了编译一次、多次执行的行为。

## 请求时间要求

OPcache 的 `file_update_protection` 功能防止缓存最近修改过的文件（默认 2 秒内）。在每个请求的初始化期间，OPcache 将文件的修改时间与当前请求时间进行比较。

OxPHP 的 SAPI 提供 `get_request_time` 回调，返回当前 Unix 时间戳。此回调在 `php_request_startup()` 期间被 PHP 调用，这意味着请求时间**必须**在此之前可用。

### 没有请求时间会怎样

如果请求时间返回 `0`（零纪元），OPcache 的文件保护检查会将每个文件的 `mtime` 与 1970 年 1 月 1 日进行比较。由于所有文件都在该日期之后修改，OPcache 认为它们"太新"而拒绝缓存。结果是 **0% 的缓存命中率** --- 每个请求都重新编译每个脚本。

OxPHP 通过实现 `get_request_time` SAPI 回调来避免这个问题，该回调返回 `SystemTime::now()` 作为具有微秒精度的 Unix 时间戳。

## 验证 OPcache 状态

创建一个诊断脚本确认 OPcache 已激活：

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

测试：

```bash
curl http://localhost:8080/opcache_check.php
# 第一次请求：未命中（脚本被编译并缓存）
curl http://localhost:8080/opcache_check.php
# 第二次请求：命中（脚本从缓存提供）
```

健康的 OPcache 安装在初始预热期后命中率会趋近 100%。

## JIT 编译

运行 PHP 8.0+ 时支持 OPcache 的 JIT 编译器。在 `php.ini` 中启用：

```ini
opcache.enable=1
opcache.jit=tracing
opcache.jit_buffer_size=64M
```

JIT 对 CPU 密集型 PHP 代码（数学运算、循环、字符串处理）收益最大。对于 I/O 密集型应用（数据库查询、API 调用），改进微乎其微。

## 推荐设置

以下设置与附带的 `oxphp.ini` 一致，针对 PHP 文件在运行时不变的生产容器部署进行了优化：

```ini
[opcache]
opcache.enable=1
opcache.enable_cli=1
opcache.memory_consumption=128
opcache.max_accelerated_files=10000
opcache.validate_timestamps=0
opcache.revalidate_freq=0
opcache.file_update_protection=0
opcache.save_comments=1
opcache.jit=tracing
opcache.jit_buffer_size=64M
```

| 设置 | 说明 |
|------|------|
| `enable_cli` | 为 OxPHP 使用的 cli-server SAPI 激活 OPcache 所必需。 |
| `memory_consumption` | 编译脚本的共享内存（MB）。如果 `opcache_get_status()` 显示可用内存不足，请增大此值。 |
| `max_accelerated_files` | 缓存脚本的最大数量。设置为高于 `.php` 文件总数的值。 |
| `validate_timestamps` | 设为 `0` 时，OPcache 在脚本被缓存后不再检查文件系统变更。你必须重启容器（或调用 `opcache_reset()`）才能应用代码更改。 |
| `revalidate_freq` | 文件修改检查之间的秒数。仅在 `validate_timestamps=1` 时适用。 |
| `file_update_protection` | 文件修改后等待多少秒才缓存。在生产环境中设为 `0` 以避免保护窗口。 |
| `save_comments` | 在缓存脚本中保留文档注释。使用基于注解路由的框架（如 Symfony、Laravel）需要此设置。 |

这些是假定 PHP 文件在容器生命周期内不可变的生产优化设置。在开发环境中，你可能需要使用 `validate_timestamps=1` 和 `revalidate_freq=2`，这样代码更改无需重启服务器即可生效。

## ZTS 与共享内存

OxPHP 以 ZTS 模式运行 PHP，每个工作线程有自己的执行上下文，但所有线程共享同一个 OPcache 共享内存段。这意味着：

- 工作线程 0 编译的脚本立即可供工作线程 1、2、3 等使用。
- OPcache 的内部锁机制安全地处理并发编译。
- 内存消耗不随工作线程数量增长 --- 每个编译脚本的一份副本服务所有线程。

这比 PHP-FPM 更节省内存，因为在 PHP-FPM 中每个进程维护自己的 OPcache 段（除非使用 `opcache.file_cache` 配合 `file_cache_only=1` 实现共享存储）。

## 另请参阅

- [PHP 扩展函数](functions.md) --- `oxphp_server_info()` 函数暴露 `request_time`
- [超全局变量](superglobals.md) --- `$_SERVER` 和其他全局变量如何在 OPcache 的 RINIT 之前填充
- [工作池](/architecture/worker-pool.md) --- ZTS 工作线程和共享内存架构
- [SAPI 桥接](/architecture/sapi-bridge.md) --- 提供 `get_request_time` 回调的 C 桥接
