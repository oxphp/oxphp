---
title: 安装
description: 通过 Docker 镜像安装 OxPHP 或从源码构建，涵盖前置条件、验证步骤和平台注意事项。
---

# 安装

OxPHP 以 Docker 镜像的形式分发 —— 这是开始提供 PHP 应用服务最快、最推荐的方式。该镜像在 Alpine Linux 上捆绑了服务器二进制文件、PHP 8.4 ZTS、OxPHP 扩展以及所有运行时依赖。

## Docker（推荐）

从 GitHub Container Registry 拉取官方镜像：

```bash
docker pull ghcr.io/oxphp/oxphp:0.1.0
```

该镜像包含：

- **OxPHP 服务器二进制文件** —— 异步 HTTP 服务器
- **PHP 8.4 ZTS** —— 支持多 Worker 执行的线程安全 PHP 运行时
- **OxPHP PHP 扩展**（`oxphp_sapi.so`）—— 提供 `oxphp_request_id()`、`oxphp_server_info()`、`oxphp_worker()` 等内置函数
- **Bridge 库**（`liboxphp_bridge.so`）—— 连接 Rust 服务器与 PHP 运行时
- **Alpine Linux** 基础镜像 —— 最小化运行时占用
- 以 **www-data**（UID 82，GID 82）用户运行，实现非 root 容器执行

在官方镜像基础上扩展，将应用容器化：

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.1.0

COPY --chown=www-data:www-data . /var/www/html
```

构建并运行：

```bash
docker build -t my-app .
docker run -p 80:80 my-app
```

服务器默认监听 `80` 端口，文档根目录为 `/var/www/html/public`。

## 源码构建（不含 PHP）

禁用 PHP 功能，从源码构建 OxPHP，仅提供静态文件服务：

```bash
cargo build --release --no-default-features
```

二进制文件位于 `target/release/oxphp`。该模式使用存根执行器，对 PHP 请求返回占位响应，同时正常提供静态文件服务。在没有 PHP 运行时的环境中测试服务器时，此模式非常有用。

## 源码构建（含 PHP）

构建完整 PHP 支持的 OxPHP，需要先编译并安装 Bridge 库和 PHP 扩展。

### 前置条件

- Rust 工具链（1.91.1 或更高版本）
- PHP 8.4（启用 ZTS，即 Zend Thread Safety）
- C 编译器（gcc 或 clang）
- `phpize` 及 PHP 开发头文件

### 构建步骤

```bash
# 1. 构建并安装 Bridge 库
cd ext/bridge
make && sudo make install

# 2. 构建并安装 PHP 扩展
cd ../
phpize && ./configure --enable-oxphp-sapi && make && sudo make install

# 3. 构建 OxPHP（默认功能包含 php）
cargo build --release
```

运行时，二进制文件需要在库搜索路径中找到共享库：

```bash
export LD_LIBRARY_PATH=/usr/local/lib
./target/release/oxphp
```

> **注意：** 部署到 Alpine Linux 时，须在运行 PHP 的同一个 `php:8.4-zts-alpine` 镜像内构建 Rust 二进制文件。混用 glibc 和 musl 构建会导致运行时错误。官方 Docker 镜像已正确处理此问题。

## 验证安装

启动 OxPHP 后，结构化 JSON 日志输出确认服务器正在运行：

```text
{"timestamp":"...","level":"INFO","message":"OxPHP HTTP server starting","listen_addr":"0.0.0.0:80",...}
{"timestamp":"...","level":"INFO","message":"Server listening","addr":"0.0.0.0:80"}
```

测试服务器是否响应：

```bash
curl http://localhost/
```

如果通过 `INTERNAL_ADDR` 启用了内部服务器，请验证健康检查端点：

```bash
curl http://localhost:9090/health
```

健康的服务器返回 `200` 及 JSON 状态信息，降级的服务器返回 `503`。

## 下一步

- [快速开始](quick-start.md) —— 创建项目，使用 Docker Compose 运行 OxPHP，并发起第一个请求
- [Docker 指南](docker.md) —— 开发和生产环境的 Dockerfile、Compose 配置和数据卷挂载
- [配置](../operations/configuration.md) —— 完整的环境变量参考
