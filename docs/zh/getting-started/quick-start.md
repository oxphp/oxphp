---
title: 快速开始
description: 5 分钟内运行 OxPHP
---

本指南将带你使用 Docker 运行 OxPHP，并提供第一个 PHP 文件的服务。

## 1. 创建项目目录

```bash
mkdir my-oxphp-app && cd my-oxphp-app
```

## 2. 创建 Dockerfile

```dockerfile
FROM ghcr.io/oxphp/oxphp:nightly

COPY --chown=www-data:www-data . /var/www/html
```

## 3. 添加 compose.yml

创建 `compose.yml`：

```yaml
services:
  oxphp:
    build: .
    ports:
      - "8080:8080"
      - "9090:9090"
    environment:
      - LISTEN_ADDR=0.0.0.0:8080
      - DOCUMENT_ROOT=/var/www/html/public
      - INTERNAL_ADDR=0.0.0.0:9090
```

## 4. 创建测试 PHP 文件

```bash
mkdir -p public
```

创建 `public/index.php`：

```php
<?php

$info = oxphp_server_info();
$requestId = oxphp_request_id();

echo "<h1>OxPHP</h1>\n";
echo "<p>Request ID: {$requestId}</p>\n";
echo "<p>SAPI: {$info['sapi']}</p>\n";
echo "<p>Version: {$info['version']}</p>\n";
echo "<p>Worker: {$info['worker_id']}</p>\n";
echo "<p>Time: " . date('c') . "</p>\n";
```

## 5. 启动服务器

```bash
docker compose up -d
```

## 6. 测试应用

在浏览器中访问 `http://localhost:8080/`，或使用 curl：

```bash
curl http://localhost:8080/
```

你应该看到类似以下的输出：

```html
<h1>OxPHP</h1>
<p>Request ID: 67a4b3c100000001</p>
<p>SAPI: oxphp</p>
<p>Version: 0.1.0</p>
<p>Worker: 0</p>
<p>Time: 2026-02-11T12:00:00+00:00</p>
```

## 7. 检查服务器健康状态

内部服务器在 9090 端口暴露健康检查和指标端点：

```bash
# 健康检查 — 返回 200 及 {"status":"ok"}
curl http://localhost:9090/health

# 兼容 Prometheus 的指标
curl http://localhost:9090/metrics

# 当前服务器配置（敏感值已脱敏）
curl http://localhost:9090/config
```

## 8. 查看日志

```bash
docker compose logs -f oxphp
```

OxPHP 输出结构化的 JSON 日志。每个请求会产生一条访问日志条目，包含请求方法、路径、状态码、响应时间和请求 ID。

## 后续步骤

- [Docker 指南](docker.md) -- compose.yml 参考、卷挂载及部署技巧
- [配置](../operations/configuration.md) -- 完整的环境变量列表
- [路由](../features/routing.md) -- Traditional、Framework 和 SPA 路由模式
- [PHP 集成](../php/functions.md) -- 可用的 PHP 扩展函数

## 参见

- [安装](installation.md) -- 源码构建说明和前置条件
- [架构概览](../architecture/overview.md) -- 运行时模型和组件图
- [Worker 池](../architecture/worker-pool.md) -- PHP worker 线程扩缩容及队列行为
