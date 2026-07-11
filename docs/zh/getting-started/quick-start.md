---
title: 快速开始
description: 5 分钟内启动 OxPHP。创建项目、编写 PHP 应用、启动服务器并发起第一个请求。
---

# 快速开始

## 一条命令

如果你已有一个包含 `public/` 目录的 PHP 项目：

```bash
docker run -p 80:80 -v .:/var/www/html ghcr.io/oxphp/oxphp:0.10.0
```

打开 `http://localhost/` —— 你的应用已在运行。

如需启用内部服务器（健康检查、指标、配置）：

```bash
docker run -p 80:80 -p 9090:9090 -e INTERNAL_ADDR=0.0.0.0:9090 -v .:/var/www/html ghcr.io/oxphp/oxphp:0.10.0
```

---

## 使用 Docker Compose 逐步搭建

更详细的设置 —— 从空目录到包含健康检查和结构化日志的可运行 PHP 应用。

### 1. 创建项目目录

```bash
mkdir my-oxphp-app && cd my-oxphp-app
```

### 2. 创建 Dockerfile

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.10.0

COPY --chown=www-data:www-data . /var/www/html
```

官方镜像包含服务器二进制文件、PHP 8.4 或 8.5 ZTS（默认 8.5；如需 8.4，请拉取 `:0.10.0-php8.4` 或任意 `*-php8.4*` 标签）、OxPHP PHP 扩展及所有运行时依赖。

> **提示：** 如果你的应用需要自定义 PHP 扩展（pdo_pgsql、intl、xdebug 等），请参阅仓库中的 [`examples/dockerfile/Dockerfile`](../../../examples/dockerfile/Dockerfile) —— 一个开箱即用的多阶段 Dockerfile，包含独立的 `dev` 和 `prod` 构建目标。

### 3. 添加 compose.yaml

```yaml
services:
  oxphp:
    build: .
    ports:
      - "80:80"
      - "9090:9090"
    environment:
      - LISTEN_ADDR=0.0.0.0:80
      - DOCUMENT_ROOT=/var/www/html/public
      - INTERNAL_ADDR=0.0.0.0:9090
      - LOG_LEVEL=info
      - ACCESS_LOG=all
```

`80` 端口提供应用服务，`9090` 端口暴露内部服务器，用于健康检查、Prometheus 指标和当前配置快照。

### 4. 创建 PHP 应用

```bash
mkdir -p public
```

创建 `public/index.php`：

```php
<?php

$requestId = oxphp_request_id();
$info      = oxphp_server_info();

echo "<h1>OxPHP</h1>\n";
echo "<p>Request ID: {$requestId}</p>\n";
echo "<p>Worker: {$info['worker_id']}</p>\n";
echo "<p>SAPI: " . php_sapi_name() . "</p>\n";
echo "<p>Version: {$info['version']}</p>\n";
echo "<p>Time: " . date('c') . "</p>\n";
```

`oxphp_request_id()` 返回每个请求的唯一 ID，`oxphp_server_info()` 返回运行中服务器的详细信息，包括 `version`、`worker_id`、`request_time` 和 `worker_mode`。

### 5. 构建并启动

```bash
docker compose up -d --build
```

### 6. 测试应用

```bash
curl http://localhost/
```

预期输出：

```html
<h1>OxPHP</h1>
<p>Request ID: 67a4b3c11a2b00000001</p>
<p>Worker: 0</p>
<p>SAPI: cli-server</p>
<p>Version: 0.10.0</p>
<p>Time: 2026-03-23T12:00:00+00:00</p>
```

每个请求都会获得一个唯一 ID，Worker ID 显示处理该请求的 PHP Worker 线程编号。

> **为什么 `php_sapi_name()` 报告 `cli-server` 而不是 `oxphp`？** OxPHP 有意将自身注册为 OPcache 已识别的 SAPI 名称之一。OPcache 对未知 SAPI 会自动禁用；如果不做这次重命名，PHP 执行将完全跳过 OPcache 层，速度会下降数倍。代价是 `php_sapi_name()` 无法用来检测 OxPHP——请改用 `function_exists('oxphp_request_id')` 或 `OxPHP\Http\Request::current()`。

### 7. 查看内部端点

```bash
# 健康检查 —— 健康时返回 200，降级时返回 503
curl http://localhost:9090/health

# Prometheus 兼容指标
curl http://localhost:9090/metrics

# 当前配置（TLS 路径已脱敏）
curl http://localhost:9090/config
```

### 8. 查看日志

```bash
docker compose logs -f oxphp
```

由于设置了 `ACCESS_LOG=all`，每个请求都会以结构化 JSON 日志行的形式记录，包含方法、路径、状态码、响应时间和请求 ID。

## 下一步

- [Docker 指南](docker.md) —— 开发和生产环境的 Dockerfile、Compose 配置、PHP ini 挂载和健康检查设置
- [配置](../operations/configuration.md) —— 完整的环境变量参考
- [路由](../features/routing.md) —— 传统模式、框架模式、SPA 模式和 Worker 路由模式
- [Worker 模式](../features/worker-mode.md) —— 启动一次即可处理多个请求的持久化 PHP 进程
- [PHP 函数](../php/functions.md) —— 所有 OxPHP 内置 PHP 函数
