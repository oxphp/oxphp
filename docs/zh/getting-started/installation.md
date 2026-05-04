---
title: 安装
description: 通过 Docker 镜像安装 OxPHP 或从源码构建，涵盖前置条件、验证步骤和平台注意事项。
---

# 安装

OxPHP 以 Docker 镜像的形式分发 —— 这是开始提供 PHP 应用服务最快、最推荐的方式。该镜像在 Alpine Linux 上捆绑了服务器二进制文件、PHP 8.4 或 8.5 ZTS、OxPHP 扩展以及所有运行时依赖。默认的 `:0.3.0` 和 `:latest` 标签随附 PHP 8.4；如需 PHP 8.5，请使用 `:0.3.0-php8.5`、`:php8.5` 或任意 `*-php8.5*` 标签变体。

## Docker（推荐）

从 GitHub Container Registry 拉取官方镜像：

```bash
docker pull ghcr.io/oxphp/oxphp:0.5.0
```

该镜像包含：

- **OxPHP 服务器二进制文件** —— 异步 HTTP 服务器
- **PHP ZTS 运行时** —— 8.4 或 8.5，取决于所拉取的标签；支持多 Worker 执行的线程安全 PHP
- **OxPHP PHP 扩展**（`oxphp_sapi.so`）—— 提供 `oxphp_request_id()`、`oxphp_server_info()`、`oxphp_worker()` 等内置函数
- **Bridge 库**（`liboxphp_bridge.so`）—— 连接 Rust 服务器与 PHP 运行时
- **Alpine Linux** 基础镜像 —— 最小化运行时占用
- 以 **www-data**（UID 82，GID 82）用户运行，实现非 root 容器执行

### 镜像结构

运行时镜像的文件布局：

```
/usr/local/
├── bin/
│   └── oxphp                                        # 服务器二进制文件
├── lib/
│   ├── libphp.so                                    # PHP ZTS 运行时（8.4 或 8.5，与镜像标签一致）
│   ├── liboxphp_bridge.so                           # C Bridge 库
│   └── php/extensions/no-debug-zts-20240924/
│       └── oxphp_sapi.so                            # OxPHP PHP 扩展
├── etc/php/
│   └── conf.d/
│       ├── oxphp.ini                                # OxPHP 的 PHP 配置
│       └── extension.ini                            # extension=oxphp_sapi.so
```

OxPHP 的三个组件及其用途：

| 组件 | 大小 | 用途 |
|------|------|------|
| `oxphp` | ~8 MB | HTTP 服务器、路由、插件、指标 |
| `liboxphp_bridge.so` | ~50 KB | 连接服务器与 PHP 运行时的共享 Bridge 库 |
| `oxphp_sapi.so` | ~200 KB | PHP 函数（`oxphp_request_id()`、`OxPHP\Http\Request` 等） |

依赖链：

```
oxphp ──► libphp.so ──► libxml2, libcurl, libsqlite3, libonig, ...
  │
  └──► liboxphp_bridge.so ◄── oxphp_sapi.so
```

`oxphp` 二进制文件链接到 `libphp.so` 和 `liboxphp_bridge.so`。PHP 扩展 `oxphp_sapi.so` 同样链接到 Bridge 库，从而使 per-request 状态可在您的 PHP 代码中使用。

### 最小化 Dockerfile

基础镜像 `php:8.4-zts-alpine3.23`（或 `php:8.5-zts-alpine3.23`）已包含 `libphp.so` 及其所有依赖。请确保 `FROM` 中的 PHP 次版本与所复制的 OxPHP 标签一致。只需复制三个 OxPHP 构件即可：

```dockerfile
FROM php:8.4-zts-alpine3.23

COPY --from=ghcr.io/oxphp/oxphp:0.5.0 /usr/local/bin/oxphp /usr/local/bin/oxphp
COPY --from=ghcr.io/oxphp/oxphp:0.5.0 /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
COPY --from=ghcr.io/oxphp/oxphp:0.5.0 /usr/local/lib/php/extensions/no-debug-zts-20240924/oxphp_sapi.so /usr/local/lib/php/extensions/no-debug-zts-20240924/

RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp.ini

COPY --chown=www-data:www-data . /var/www/html/public

EXPOSE 80 443 9090

CMD ["oxphp"]
```

此方式适合开发场景 —— 可使用 PHP CLI、`composer`、`docker-php-ext-install`、`xdebug`。详情参阅 [Docker 指南](docker.md)。

### 生产环境 Dockerfile

官方 OxPHP 镜像非常精简 —— 不包含 PHP CLI 和扩展构建工具。如果应用需要额外扩展（pdo_mysql、intl 等），请在单独的构建阶段编译，然后复制到最终镜像中：

```dockerfile
# 扩展构建阶段
FROM php:8.4-zts-alpine3.23 AS extensions

RUN apk add --no-cache icu-dev postgresql-dev \
    && docker-php-ext-install pdo pdo_mysql pdo_pgsql intl

# 生产环境
FROM ghcr.io/oxphp/oxphp:0.5.0

# 扩展的运行时依赖
USER root
RUN apk add --no-cache icu-libs libpq

# 复制已编译的扩展
COPY --from=extensions /usr/local/lib/php/extensions/no-debug-zts-20240924/*.so /usr/local/lib/php/extensions/no-debug-zts-20240924/

# 启用扩展
RUN { \
        echo "extension=pdo.so"; \
        echo "extension=pdo_mysql.so"; \
        echo "extension=pdo_pgsql.so"; \
        echo "extension=intl.so"; \
    } > /usr/local/etc/php/conf.d/app-extensions.ini

USER www-data

COPY --chown=www-data:www-data . /var/www/html/public
```

如果应用不需要额外扩展，以下配置即可：

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.5.0

COPY --chown=www-data:www-data . /var/www/html/public
```

构建并运行：

```bash
docker build -t my-app .
docker run -p 80:80 my-app
```

服务器默认监听 `80` 端口，文档根目录为 `/var/www/html/public` —— 上面的代码片段将项目直接复制到该目录。对于 Laravel、Symfony 等已包含 `public/` 子目录的框架，请改用 `COPY --chown=www-data:www-data . /var/www/html`，使框架自带的 `public/` 对齐默认根目录。如果项目结构进一步不同，请通过 `DOCUMENT_ROOT` 环境变量覆盖文档根目录。

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
- PHP 8.4 或 8.5（启用 ZTS，即 Zend Thread Safety）
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

> **注意：** 部署到 Alpine Linux 时，须在运行 PHP 的同一个 `php:{8.4,8.5}-zts-alpine` 镜像内构建 Rust 二进制文件 —— 次版本应与所发布的 OxPHP 镜像标签一致。混用 glibc 和 musl 构建会导致运行时错误。官方 Docker 镜像已正确处理此问题。

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
