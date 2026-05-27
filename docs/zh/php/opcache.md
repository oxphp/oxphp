---
title: OPcache 与 JIT
description: 为 OxPHP 配置 OPcache 和 JIT 编译以获得最佳 PHP 性能，包括预加载和开发环境设置。
---

# OPcache 与 JIT

OPcache 开箱即用，无需额外配置即可与 OxPHP 协同工作。所有 PHP Worker 线程共享同一块 OPcache 内存段——脚本在首次执行时编译一次，此后所有 Worker 均从缓存中提供服务。无需任何特殊设置即可启用此共享机制。

## OPcache 与 OxPHP 的工作原理

OxPHP 将自身注册为具名 SAPI，OPcache 将其与其他服务器 SAPI 同等对待。主要特性如下：

- **跨 Worker 共享缓存**：所有 PHP Worker 线程使用同一份编译后的操作码缓存。一个 Worker 编译文件后，所有 Worker 均可受益。
- **无逐请求编译**：每个脚本首次请求后，后续请求完全跳过解析和编译步骤。
- `opcache.enable_cli` 不影响 OxPHP——该设置仅适用于名为 `cli` 和 `phpdbg` 的 SAPI。OxPHP 注册的 SAPI 名称为 `cli-server`，因此 OPcache 仅通过 `opcache.enable` 控制。如果你在同一容器中使用 PHP CLI（例如运行迁移或 Artisan 命令），`opcache.enable_cli` 参数会很有用。官方 OxPHP 镜像随服务器二进制文件一同提供 PHP CLI，因此如果你的 CLI 脚本能从缓存中受益，可以设置 `opcache.enable_cli=1`。

要启用 OPcache，至少需要以下配置：

```ini
[opcache]
opcache.enable=1
```

> **注意：** 官方 OxPHP Docker 镜像基于 `php:*-zts-alpine`，该基础镜像将 OPcache 静态编译进 PHP 二进制文件。**请勿**在 INI 文件中添加 `zend_extension=opcache`——该扩展已加载，添加此行将在每次 PHP 启动时产生警告。只需 `[opcache]` 配置节即可。

## 推荐生产环境配置

以下配置针对 PHP 文件在运行时不会更改的生产容器部署场景进行了优化。禁用时间戳验证并在启动时预加载编译后的文件，以获得最大吞吐量。

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

| 配置项 | 推荐值 | 描述 |
|--------|--------|------|
| `memory_consumption` | `128` | 编译脚本使用的共享内存（MB）。若 `opcache_get_status()` 显示可用内存不足，请增大此值。 |
| `interned_strings_buffer` | `16` | 所有 Worker 共享的驻留字符串内存（MB）。 |
| `max_accelerated_files` | `10000` | 可缓存脚本的最大数量。应设置为高于项目中 `.php` 文件的总数。 |
| `validate_timestamps` | `0` | 设为 `0` 时，OPcache 不检查文件系统变更。重启容器或调用 `opcache_reset()` 以使代码变更生效。 |
| `revalidate_freq` | `0` | 文件系统检查的间隔秒数。当 `validate_timestamps=0` 时无效。 |
| `file_update_protection` | `0` | 文件修改后多少秒才允许缓存该文件。设为 `0` 以在启动时立即缓存。 |

## 开发环境配置

在开发环境中，启用时间戳验证以使代码变更无需重启容器即可生效。禁用 JIT 以在调试时获得更清晰的堆栈追踪信息。

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

设置 `validate_timestamps=1` 后，OPcache 每隔 `revalidate_freq` 秒检查一次文件修改时间。这会带来少量的逐请求开销，但允许你编辑 PHP 文件后在下次请求时立即看到变更。

**这是 OxPHP 在开发模式下推荐的代码热重载方案。** OPcache 在每次 include 时内联执行检查，因此代码编辑会在下次请求时被自动加载，无需重启容器，也无需外部文件监视守护进程。`revalidate_freq=0` 表示每次 include 都立即 stat 检查（精度最高，I/O 稍多）；`revalidate_freq=2` 可摊销 stat 开销——上面示例中的默认值是一个合理的折中，特别是当 `DOCUMENT_ROOT` 挂载在较慢的 bind-mount 上时（macOS/Windows 上的 Docker）。

### `validate_timestamps` 不会重载的内容

即使设置了 `validate_timestamps=1`，仍有几类变更需要重启容器（或回收 Worker）才能生效：

- **预加载文件**（`opcache.preload`）在服务器启动时被链接进来，永远不会被重新验证。编辑 preload 文件后——重启容器。
- **Worker 模式的 bootstrap 状态** ——在 [Worker 模式](../features/worker-mode.md) 中，自动加载器、DI 容器以及在外层作用域构建的所有对象都驻留在 Worker 内存中。OPcache 会重新编译已更改的类文件，但 Worker 不会重新执行其 bootstrap。在开发循环中，可在每次请求末尾调用 [`Worker::scheduleExit()`](worker-class.md#scheduleexit)（例如挂在 `OXPHP_DEV` 环境标记之下），让 Worker 回收并重新运行外层作用域以加载所有变更。
- **框架级缓存** ——编译后的 Symfony 容器、Laravel 路由/配置/视图缓存、Composer 优化过的 classmap。它们是 `.php` 文件，OPcache 确实会重新验证，但其中的值仍指向过期的类路径或容器 ID。请运行框架的 `cache:clear` 命令——仅靠 OPcache 是不够的。
- **非 PHP 文件** —— `.env`、`composer.json`、YAML/JSON 配置、在 OPcache 之外编译的模板。OPcache 只跟踪它自己编译过的文件；其他所有文件都需要重启。

## JIT 编译

OPcache 的 JIT 编译器在运行时将 PHP 操作码转译为本机机器码。建议使用 `tracing` 模式以获得最佳优化效果：

```ini
opcache.jit=tracing
opcache.jit_buffer_size=64M
```

JIT 对 CPU 密集型 PHP 代码提升最为显著——计算密集型循环、字符串处理、图像处理和模板渲染。对于大部分时间花费在等待数据库查询或外部 API 调用上的 I/O 密集型应用，提升效果有限。

禁用 JIT：

```ini
opcache.jit=disable
opcache.jit_buffer_size=0
```

## 预加载

OPcache 预加载会在服务器启动时、处理任何请求之前编译并缓存 PHP 文件。这完全消除了首次请求的编译开销，并使类和函数无需任何 `require` 或自动加载开销即可全局使用。

在 INI 文件中配置预加载：

```ini
opcache.preload=/var/www/html/preload.php
opcache.preload_user=www-data
```

创建一个 `preload.php` 脚本，加载最常用的文件：

```php
<?php
// preload.php — 在服务器启动时运行一次

require __DIR__ . '/vendor/autoload.php';

// 预加载框架核心文件
$files = glob(__DIR__ . '/vendor/symfony/http-kernel/**.php');
foreach ($files as $file) {
    opcache_compile_file($file);
}

// 预加载热点应用路径
opcache_compile_file(__DIR__ . '/src/Controller/ApiController.php');
opcache_compile_file(__DIR__ . '/src/Service/UserService.php');
```

> **注意：** 预加载的类和函数对所有请求永久可用。在重启服务器之前无法更改它们。

> **Worker 模式与预加载：** 如果你使用 [Worker 模式](../features/worker-mode.md)，应用已经只初始化一次——自动加载器、配置和数据库连接在请求之间保持不变。OPcache 预加载对此形成补充，消除了操作码编译的开销，但不能替代应用初始化。两种机制独立工作，可以同时使用。

## 应用 PHP 配置

OxPHP 从标准的 `conf.d` 目录读取 PHP 配置。使用 Docker 卷挂载或 `COPY` 指令提供自定义 INI 文件。

**Docker run：**

```bash
docker run -p 80:80 \
  -v ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro \
  ghcr.io/oxphp/oxphp:0.6.0
```

**Dockerfile：**

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.6.0

COPY oxphp.ini /usr/local/etc/php/conf.d/oxphp.ini
COPY --chown=www-data:www-data . /var/www/html
```

**Docker Compose：**

```yaml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.6.0
    ports:
      - "80:80"
    volumes:
      - ./oxphp.ini:/usr/local/etc/php/conf.d/oxphp.ini:ro
      - ./src:/var/www/html
```

## 监控缓存状态

从 PHP 中检查实时 OPcache 状态以验证其正常工作：

```php
<?php
$status = opcache_get_status();

echo "Cached scripts: " . $status['opcache_statistics']['num_cached_scripts'] . "\n";
echo "Cache hits: "     . $status['opcache_statistics']['hits'] . "\n";
echo "Cache misses: "   . $status['opcache_statistics']['misses'] . "\n";
echo "Free memory: "    . $status['memory_usage']['free_memory'] . " bytes\n";
```

若 `free_memory` 持续偏低，请增大 `opcache.memory_consumption` 的值。

## 参见

- [Docker 指南](../getting-started/docker.md) -- 容器配置与挂载配置文件
- [配置参考](../operations/configuration.md) -- OxPHP 的环境变量
- [Worker 模式](../features/worker-mode.md) -- 从 OPcache 中获益最多的持久 PHP 进程
