---
title: Docker 指南
description: 使用 Docker 运行 OxPHP，涵盖最小化和多阶段 Dockerfile、Compose 配置、PHP ini 挂载、健康检查和端口说明。
---

# Docker 指南

OxPHP 专为容器化运行而设计。本指南涵盖使用 Docker 构建、配置和运行 OxPHP 所需的一切内容 —— 从最简单的单阶段镜像，到包含独立开发和生产目标的完整多阶段方案。

## 最小化 Dockerfile

将应用容器化最简单的方式：

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.9.0

COPY --chown=www-data:www-data . /var/www/html/public
```

这会将应用复制到容器中。默认 `DOCUMENT_ROOT` 为 `/var/www/html/public`；对于 Laravel、Symfony 或任何已包含 `public/` 子目录的项目，请改用 `COPY --chown=www-data:www-data . /var/www/html`，使框架自带的 `public/` 对齐默认值。服务器默认监听 `80` 端口。

## 多阶段 Dockerfile

对于实际生产应用，建议使用带有独立 `dev` 和 `prod` 目标的多阶段 Dockerfile。`dev` 目标包含 PHP CLI、Composer 和 Xdebug，`prod` 目标基于最小化 OxPHP 镜像，仅包含生产所需的内容。

> **提示：** 仓库中的 [`examples/dockerfile/Dockerfile`](../../../examples/dockerfile/Dockerfile) 提供了该 Dockerfile 的可直接使用版本。将其复制到你的项目中，按需调整扩展即可。

```dockerfile
# ── Stage: php-base — shared PHP extensions ──────────────────
FROM php:8.4-zts-alpine3.23 AS php-base

RUN apk add --no-cache \
        icu-dev \
        icu-libs \
        postgresql-dev \
        libpq \
    && docker-php-ext-install \
        pdo \
        pdo_mysql \
        pdo_pgsql \
        intl \
    && apk del icu-dev postgresql-dev

# ── Stage: php-dev — add Xdebug on top of base ───────────────
FROM php-base AS php-dev

RUN apk add --no-cache $PHPIZE_DEPS linux-headers \
    && pecl install xdebug \
    && docker-php-ext-enable xdebug \
    && apk del $PHPIZE_DEPS linux-headers

# ── Stage: composer ───────────────────────────────────────────
FROM composer:2 AS composer

# ── Stage: oxphp — pull OxPHP artifacts ──────────────────────
FROM ghcr.io/oxphp/oxphp:0.9.0 AS oxphp

# ── Target: dev ──────────────────────────────────────────────
# Includes: PHP CLI, Composer, Xdebug, OxPHP binary + extension
FROM php-dev AS dev

RUN apk add --no-cache libgcc

# Composer
COPY --from=composer /usr/bin/composer /usr/local/bin/composer

# OxPHP binary
COPY --from=oxphp /usr/local/bin/oxphp /usr/local/bin/oxphp

# Bridge library
COPY --from=oxphp /usr/local/lib/liboxphp_bridge.so /usr/local/lib/

# OxPHP PHP extension
RUN EXT_DIR=$(php -r 'echo ini_get("extension_dir");') && \
    echo "$EXT_DIR" > /tmp/ext_dir
COPY --from=oxphp /usr/local/lib/php/extensions/ /tmp/oxphp-ext/
RUN cp /tmp/oxphp-ext/*/oxphp_sapi.so "$(cat /tmp/ext_dir)/" && \
    rm -rf /tmp/oxphp-ext /tmp/ext_dir

# PHP config
RUN echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp-ext.ini

# Dev-friendly OPcache (validates timestamps)
RUN { \
        echo "[opcache]"; \
        echo "opcache.enable=1"; \
        echo "opcache.enable_cli=1"; \
        echo "opcache.validate_timestamps=1"; \
        echo "opcache.revalidate_freq=0"; \
    } > /usr/local/etc/php/conf.d/opcache-dev.ini

# Xdebug — connect back to host
RUN { \
        echo "[xdebug]"; \
        echo "xdebug.mode=debug"; \
        echo "xdebug.start_with_request=trigger"; \
        echo "xdebug.client_host=host.docker.internal"; \
        echo "xdebug.client_port=9003"; \
    } > /usr/local/etc/php/conf.d/xdebug-config.ini

RUN adduser -D -H -u 82 -G www-data -s /sbin/nologin www-data 2>/dev/null || true
RUN mkdir -p /var/www/html/public && chown -R www-data:www-data /var/www/html

ENV LD_LIBRARY_PATH=/usr/local/lib

# Framework layout: project ships a public/ subdir (Laravel/Symfony/Slim).
# For a bare index.php at the project root, copy into /var/www/html/public instead.
COPY --chown=www-data:www-data . /var/www/html

EXPOSE 80 443

CMD ["oxphp"]

# ── Stage: prod-extensions — compile extensions for prod ─────
FROM php-base AS prod-extensions

RUN EXT_DIR=$(php -r 'echo ini_get("extension_dir");') && \
    mkdir -p /ext-out && \
    cp "$EXT_DIR"/pdo.so \
       "$EXT_DIR"/pdo_mysql.so \
       "$EXT_DIR"/pdo_pgsql.so \
       "$EXT_DIR"/intl.so \
       /ext-out/

# ── Target: prod — minimal, based on OxPHP image ─────────────
FROM oxphp AS prod

USER root
RUN apk add --no-cache icu-libs libpq

COPY --from=prod-extensions /ext-out/*.so /usr/local/lib/php/extensions/no-debug-zts-20240924/

RUN { \
        echo "extension=pdo_mysql.so"; \
        echo "extension=pdo_pgsql.so"; \
        echo "extension=intl.so"; \
    } > /usr/local/etc/php/conf.d/app-extensions.ini

# Framework layout: project ships a public/ subdir (Laravel/Symfony/Slim).
# For a bare index.php at the project root, copy into /var/www/html/public instead.
COPY --chown=www-data:www-data . /var/www/html

USER www-data

EXPOSE 80 443

CMD ["oxphp"]
```

构建各目标镜像：

```bash
# 开发镜像（包含 PHP CLI、Composer、Xdebug）
docker build --target dev -t myapp:dev .

# 生产镜像（最小化）
docker build --target prod -t myapp:prod .
```

> **注意：** `dev` 目标基于 `php:8.4-zts-alpine`（如需匹配 OxPHP 的 `:*-php8.5*` 变体，请将 `8.4` 替换为 `8.5`）并复制了 OxPHP 相关内容，可完整使用 PHP CLI 和 Composer。`prod` 目标直接基于 OxPHP 镜像，保持生产镜像体积精简。

## 在生产环境中安装 PHP 扩展

从 OxPHP 0.3.0 开始，生产镜像从 `php:8.4-zts-alpine`（或 `php:8.5-zts-alpine`，对应 `:*-php8.5*` 变体）继承了完整的 PHP 工具链（`php`、`docker-php-ext-install`、`phpize`），并且**不**设置 `USER` 指令。下游 Dockerfile 可以直接安装 PHP 扩展 —— 无需切换 `USER`。

### 快速开始（单阶段）

最简可用示例：

```dockerfile
FROM ghcr.io/oxphp/oxphp:0.9.0

RUN docker-php-ext-install mysqli pdo_mysql

COPY --chown=www-data:www-data . /var/www/html/public

CMD ["oxphp"]
```

`COPY` 上的 `--chown=www-data:www-data` 很重要：文件在镜像中归 `www-data`（uid 82）所有，这样编排层通过 `--user www-data` 降权后，非特权进程能读（以及在需要时写）到 webroot。

容器以 root 启动，随后 `oxphp serve` 会在服务任何流量之前默认降权到 `www-data`（见下方安全说明）。若你需要特定 uid 或非 `www-data` 用户，可在编排层显式固定运行身份。

### 最佳实践（双阶段，更小的镜像）

为获得最小的最终镜像，请在独立的构建阶段编译扩展，仅将编译好的 `.so` 文件复制到运行时阶段。上方的 [多阶段 Dockerfile](#多阶段-dockerfile) 使用 `FROM ghcr.io/oxphp/oxphp:0.9.0 AS prod` —— 简单、可移植，推荐作为起点。

仓库中的 `examples/dockerfile/Dockerfile` 走得更远：其 `prod` 目标基于纯净 `alpine`，通过显式 `apk` 依赖清单，只把 `oxphp` 二进制、`libphp.so`、编译好的 PHP 扩展以及必要的共享库复制进镜像。基础镜像从 ~188 MB 降到 ~76 MB（约 60%，不含应用代码），代价是需要在 PHP/Alpine 升级时维护 `apk` 清单。同一文件还提供 `prod-cli` 目标 —— 用于 `php artisan migrate`、Composer 以及其他不应留在 serving 路径上的短期维护命令。

注意：上方的说明中仍保留显式的 `USER root` / `USER www-data` 切换以实现纵深防御 —— 在 v0.3.0 中这些切换是可选的，因为基础镜像不再设置 `USER`。

### 运行 CLI 工具与迁移

同一生产镜像可以运行 `php` CLI 命令用于迁移、Composer 或临时检查。`docker run` 会以传入的命令替换默认 CMD —— 容器运行该命令并退出，不会同时启动 OxPHP 服务器：

```bash
# 针对生产镜像运行 Laravel 迁移。
# 容器默认以 root 运行 —— CLI 对 root 所有的挂载卷拥有写权限。
docker run --rm \
    -v "$(pwd):/var/www/html" \
    ghcr.io/oxphp/oxphp:0.9.0 \
    php artisan migrate

# 如果挂载卷归 www-data 所有，使用 Docker 的 --user：
docker run --rm --user www-data \
    -v "$(pwd):/var/www/html" \
    ghcr.io/oxphp/oxphp:0.9.0 \
    php artisan migrate
```

`docker exec <container> docker-php-ext-install <ext>` 对运行中的容器同样可用，无需额外参数 —— 适合调试一个运行中的容器。生产环境中请把扩展写进 Dockerfile，使其在重启后依然存在。

### 安全说明

生产镜像没有 `USER` 指令，因此容器**以 root 启动**（与 `nginx:alpine` / `php:*-fpm-alpine` / `frankenphp:alpine` 的约定一致）。以 root 启动让 OxPHP 能绑定特权端口 —— 但这不再意味着流量*以 root 被服务*：**`oxphp serve` 和 `oxphp run` 默认会降权到 `www-data`**，先以 root 绑定，然后在处理任何请求或启动任何 PHP worker 之前永久降权。在官方镜像（自带 `www-data` 账户）上，这是开箱即用的，无需任何编排层配置。

你仍然可以在编排层显式固定运行身份 —— 当你需要特定 uid、额外的纵深防御，或非 `www-data` 用户时，推荐这样做：

- **Docker：** `docker run --user www-data ghcr.io/oxphp/oxphp:0.9.0`
- **Compose：**
  ```yaml
  services:
    oxphp:
      image: ghcr.io/oxphp/oxphp:0.9.0
      user: www-data
  ```
- **Kubernetes：**
  ```yaml
  securityContext:
    runAsNonRoot: true
    runAsUser: 82
    runAsGroup: 82
  ```
  `runAsNonRoot: true` 是纵深防御：若 `runAsUser` 被删除或覆盖为 `0`，kubelet 会直接拒绝该 Pod，而不是默默以 root 运行。

当你以这种方式以非 root 启动容器时，OxPHP 已经是非特权的，默认的自降权变为 no-op —— 但此时进程无法绑定 1024 以下的端口（见[在 80 端口以非 root 运行](#在-80-端口以非-root-运行serve---user)，以同时保留特权绑定*和*非 root 服务）。若要有意继续以 root 服务，请传入 `oxphp serve --user=root`。

`www-data` 用户（uid 82，gid 82）由基础镜像预先创建，且 `/var/www/html` 在构建时已 `chown` 给它，所以上述任意降权路径 —— 包括默认自降权 —— 都指向可读的 webroot。

> 像 `docker run … php artisan migrate` 这样的 CLI 调用直接运行 `php` 二进制文件，而非 `oxphp serve`/`run`，因此它们**不会**自降权 —— 它们以容器的启动用户（默认 root）运行。对于这些命令，请使用上面所示的 Docker `--user`。

### 在 80 端口以非 root 运行（`serve --user`）

在编排层降权（见上）有一个限制：以 `www-data` 启动的进程无法绑定特权端口（低于 1024）。否则要监听 `:80`/`:443`，你需要 `CAP_NET_BIND_SERVICE`、类似 `su-exec` 的 entrypoint，或在端口映射后使用高端口（如 `:8080`）。

OxPHP 把这些收拢到单个进程里：它 **以 root** 绑定监听套接字，然后在接受任何连接或启动任何 PHP worker **之前**永久降权。你既得到特权端口，又让请求处理以非 root 运行，且无需额外 capabilities。**默认降权到的用户就是 `www-data`**，所以在官方镜像上，仅仅以 root 启动容器就已经能得到由 `www-data` 服务的特权绑定 —— 无需任何标志。仅当要降权到*其他*用户时才使用 `--user=<spec>`；用 `--user=root` 保持 root。

**以 root 启动容器** —— 同时不要设置 `user:`，因为绑定需要 root。下面的示例显式传入 `--user=www-data` 以便自文档化，但它与默认行为一致：

```yaml
services:
  oxphp:
    image: ghcr.io/oxphp/oxphp:0.9.0
    command: ["oxphp", "serve", "--user=www-data"]
    ports:
      - "80:80"
      - "443:443"
    environment:
      - LISTEN_ADDR=0.0.0.0:80
```

`<spec>` 接受用户名、`name:group`、数字 `uid` 或 `uid:gid`。降权按 `initgroups → setgid → setuid` 执行，并校验无法重新获得 root，且不可逆；在 Linux 上还会设置 `no_new_privs`。**显式**的 `--user` 是 fail-fast 的：如果进程**不是**以 root 启动，`serve --user` 会报错退出，而不是悄悄以 root 继续运行。（而*默认*降权是 best-effort 的 —— 以非 root 启动时它会直接跳过，因为没有可降的权限。）

> 只选**一种**模型，不要两者并用。当 `:80` 由高端口或外部负载均衡器终结时，用编排层降权（`user:` / `runAsUser`）；当你希望由 OxPHP 自己掌管特权绑定时，用 `serve --user`。同时设置 `user:` **和** `serve --user` 会导致绑定失败——此时容器已不是 root。

**降权用户的文件权限清单。** 降权后，OxPHP 运行时接触的一切都必须对 `<spec>` 可访问：

| 资源 | 要求 |
|------|------|
| `DOCUMENT_ROOT` | 可读。`/var/www/html` 在构建镜像时已 `chown` 给 `www-data`，因此默认即满足。 |
| 会话保存路径（`session.save_path`，默认 `/tmp`） | 可写。 |
| 上传临时目录（`upload_tmp_dir`） | 使用文件上传时需可写。 |
| OPcache 文件缓存（`opcache.file_cache`） | 启用二级文件缓存时需可写。 |
| 基于文件的访问日志 | 当日志写入文件而非 stdout 时需可写。 |
| TLS 私钥（`TLS_KEY`） | **降权用户需可读** —— 对组或所有人可读，而非仅 root 的 `0600`。私钥在降权*之后*读取，因此仅 root 可读的私钥会导致 TLS 启动失败。 |

## Docker Compose

### 生产环境

```yaml
services:
  oxphp:
    build:
      context: .
      target: prod
    ports:
      - "80:80"
      - "443:443"
      - "9090:9090"
    environment:
      - LISTEN_ADDR=0.0.0.0:80
      - DOCUMENT_ROOT=/var/www/html/public
      - ENTRY_FILE=index.php
      - INTERNAL_ADDR=0.0.0.0:9090
      - LOG_LEVEL=info
      - ACCESS_LOG=error
      - PHP_WORKERS=4
      - DRAIN_TIMEOUT_SECONDS=25
      - COMPRESSION_LEVEL=4
    restart: unless-stopped
```

### 开发环境

将源码目录挂载为数据卷，文件变更无需重新构建即可生效。`dev` 目标启用了 OPcache 时间戳验证，PHP 会自动感知文件变更。

```yaml
services:
  oxphp:
    build:
      context: .
      target: dev
    ports:
      - "80:80"
      - "9090:9090"
    volumes:
      - ./src:/var/www/html:ro
      - ./custom.ini:/usr/local/etc/php/conf.d/custom.ini:ro
    environment:
      - LISTEN_ADDR=0.0.0.0:80
      - DOCUMENT_ROOT=/var/www/html/public
      - ENTRY_FILE=index.php
      - INTERNAL_ADDR=0.0.0.0:9090
      - LOG_LEVEL=debug
      - ACCESS_LOG=all
```

## 数据卷挂载

| 宿主机路径 | 容器路径 | 用途 |
|-----------|---------|------|
| `./src` | `/var/www/html` | 应用文件（PHP 脚本、静态资源）。生产环境建议使用 `:ro` |
| `./custom.ini` | `/usr/local/etc/php/conf.d/custom.ini` | PHP 运行时配置（OPcache、会话、JIT）。建议使用 `:ro` |
| `./certs` | `/etc/ssl/oxphp` | TLS 证书和私钥。建议使用 `:ro` |

## 端口说明

| 端口 | 环境变量 | 用途 |
|------|---------|------|
| `80` | `LISTEN_ADDR` | 主 HTTP 服务器 |
| `443` | `LISTEN_ADDR` | 主 HTTPS 服务器（配置 TLS 后生效） |
| `9090` | `INTERNAL_ADDR` | 内部服务器：`/health`、`/metrics`、`/config` |

> **注意：** 内部服务器默认禁用，设置 `INTERNAL_ADDR` 即可启用。在生产环境中，应确保内部端口仅对编排系统或监控系统可访问，不要公开暴露。

## PHP 配置

创建 `custom.ini` 文件并将其挂载到容器中，可自定义 PHP 设置。这是配置 OPcache、JIT、会话及其他 PHP 运行时参数的推荐方式。

```ini
; 请勿添加 zend_extension=opcache —— OPcache 已静态编译到
; PHP ZTS 基础镜像中，下方 [opcache] 节直接配置即可。

[opcache]
opcache.enable = 1
opcache.enable_cli = 1
opcache.memory_consumption = 128
opcache.interned_strings_buffer = 16
opcache.max_accelerated_files = 10000
opcache.validate_timestamps = 0
opcache.jit_buffer_size = 64M
opcache.jit = tracing

[Session]
session.save_path = /tmp
session.use_cookies = 1
session.use_only_cookies = 1
```

> **注意：** 请勿在此文件中添加 `zend_extension=opcache`。OPcache 已内置于 OxPHP 所使用的 PHP ZTS 镜像中，添加 `zend_extension` 行将在每次请求启动时产生警告。

在开发环境中，设置 `opcache.validate_timestamps = 1` 和 `opcache.revalidate_freq = 0`，PHP 将在不重启容器的情况下感知文件变更。

更多推荐设置和 JIT 配置，请参阅 [OPcache](../php/opcache.md)。

## 健康检查

添加 Docker 健康检查，让 Docker 或编排系统监控容器健康状态。此功能需要设置 `INTERNAL_ADDR`。

在 `compose.yaml` 中：

```yaml
services:
  oxphp:
    environment:
      - INTERNAL_ADDR=0.0.0.0:9090
    healthcheck:
      test: ["CMD", "wget", "--quiet", "--tries=1", "--spider", "http://localhost:9090/health"]
      interval: 10s
      timeout: 5s
      retries: 3
      start_period: 5s
```

在 `Dockerfile` 中：

```dockerfile
HEALTHCHECK --interval=10s --timeout=5s --retries=3 --start-period=5s \
    CMD wget --quiet --tries=1 --spider http://localhost:9090/health || exit 1
```

`/health` 端点在服务器健康时返回 `200`，降级时返回 `503`。响应 JSON 包含运行时长、总请求数和当前连接数。在 Kubernetes 中，可将同一端点同时用作存活探针和就绪探针。

## 下一步

- [配置](../operations/configuration.md) —— 完整的环境变量参考
- [路由](../features/routing.md) —— 传统模式、框架模式、SPA 模式和 Worker 路由模式
- [Worker 模式](../features/worker-mode.md) —— 适用于框架应用的持久化 PHP 进程
- [TLS](../features/tls.md) —— 内置 TLS 终止实现 HTTPS
- [健康检查](../operations/health-checks.md) —— 健康端点详情和 Kubernetes 集成
- [优雅关闭](../operations/graceful-shutdown.md) —— 排空行为和关闭顺序
