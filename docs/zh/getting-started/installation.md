---
title: 安装
description: 如何安装和运行 OxPHP
---

## Docker 镜像（推荐）

OxPHP 以预构建的 Docker 镜像形式发布。拉取最新的每夜构建版本：

```bash
docker pull ghcr.io/oxphp/oxphp:nightly
```

在项目根目录创建一个 `Dockerfile`：

```dockerfile
FROM ghcr.io/oxphp/oxphp:nightly

COPY --chown=www-data:www-data . /var/www/html
```

构建并运行：

```bash
docker build -t my-app .
docker run -p 8080:8080 my-app
```

就这些。镜像中包含 Rust 二进制文件、PHP 8.4 ZTS 运行时、bridge 库、PHP 扩展以及所有必要依赖项，无需任何构建工具。

## 前置条件

**Docker（推荐）：**

- Docker Engine 20.10+ 或 Docker Desktop

**源码构建（不含 PHP）：**

- Rust 工具链 1.75+（推荐使用 `rustup`）

**源码构建（含 PHP）：**

- Rust 工具链 1.75+
- 启用了 ZTS（Zend Thread Safety）的 PHP 8.4
- `libphp.so` 位于库搜索路径中
- 用于构建 bridge 库和 PHP 扩展的 C 编译器（gcc 或 clang）

## 源码构建（Stub 执行器）

若要在不支持 PHP 的情况下构建 OxPHP（仅提供静态文件服务，适用于开发环境），请使用 `--no-default-features` 禁用 `php` feature：

```bash
cargo build --release --no-default-features
```

生成的二进制文件位于 `target/release/oxphp`。它使用 stub 执行器，对 PHP 请求返回占位响应。

**注意：** `php` feature 默认启用。不带 `--no-default-features` 直接运行 `cargo build --release` 需要主机上已安装 `libphp.so` 和 bridge 库。

## 源码构建（含 PHP）

含 PHP 的构建需要在主机上安装 `libphp.so`（ZTS 构建）和 bridge 库：

```bash
# 构建并安装 bridge 库
cd ext/bridge
make && sudo make install

# 构建并安装 PHP 扩展
cd ext
phpize && ./configure --enable-oxphp-sapi && make && sudo make install

# 构建含 PHP 支持的 OxPHP（默认 feature 包含 php）
cargo build --release
```

运行时，二进制文件需要在库搜索路径中找到 `libphp.so` 和 `liboxphp_bridge.so`：

```bash
export LD_LIBRARY_PATH=/usr/local/lib
./target/release/oxphp
```

### Alpine 兼容性

若要部署到 Alpine Linux，必须在与 PHP 运行时相同的 `php:8.4-zts-alpine` 镜像内构建 Rust 二进制文件。在独立镜像中或不同 libc（glibc 与 musl）上构建会导致运行时 TLS 损坏。提供的 Dockerfile 已正确处理了这一问题。

## 运行测试

在未安装 PHP 的主机上，通过禁用默认 feature 来运行测试套件：

```bash
# 全量检查（格式、lint、测试）
cargo fmt -- --check && cargo clippy --no-default-features -- -D warnings && cargo test --no-default-features

# 仅单元测试
cargo test --no-default-features --lib

# 全部测试（单元测试 + 集成测试）
cargo test --no-default-features

# 含示例插件
cargo clippy --no-default-features --features plugin-example -- -D warnings && cargo test --no-default-features --features plugin-example
```

## 验证安装

启动 OxPHP 后，应看到结构化的 JSON 日志输出：

```
{"timestamp":"...","level":"INFO","message":"OxPHP HTTP server starting","listen_addr":"0.0.0.0:8080",...}
{"timestamp":"...","level":"INFO","message":"Server listening","addr":"0.0.0.0:8080"}
```

测试服务器是否响应：

```bash
curl http://localhost:8080/
```

如果配置了内部服务器，检查健康检查端点：

```bash
curl http://localhost:9090/health
```

## 参见

- [快速开始](quick-start.md) -- 5 分钟内运行 OxPHP
- [Docker](docker.md) -- compose.yml 参考、Dockerfile 阶段及部署技巧
- [配置](../operations/configuration.md) -- 完整的环境变量列表
