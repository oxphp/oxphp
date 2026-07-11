---
title: 部署示例
description: 在 OxPHP 上运行流行 PHP 框架与 CMS 的实战配方 —— Laravel、Symfony、Yii3、WordPress、Drupal、Craft、Magento、OpenCart 与 October CMS —— 每个都附带 Dockerfile、Compose 文件、安装步骤，以及关键的 OxPHP 专属说明。
---

# 部署示例

这些指南展示了如何在 OxPHP 上运行九款流行的 PHP 应用，每一款都是一个自包含的 Docker Compose 项目。每个配方都经过端到端构建与验证：店面、后台管理面板、静态资源以及 OxPHP 内部健康检查端点全部返回 `200`。

每个页面都是一份完整、可直接复制粘贴的配方 —— 包含 `Dockerfile`、`docker-compose.yml`、安装命令，以及应用自带文档（为 nginx + PHP-FPM 编写）未涵盖的 OxPHP 专属细节。

## 这些应用

| 应用 | 类型 | 路由模式 | PHP | 额外服务 | 安装方式 |
|-------------|------|--------------|-----|----------------|----------------|
| [Laravel](framework/laravel.md) | 框架 | Framework | 8.5 | MySQL | `composer create-project` |
| [Symfony](framework/symfony.md) | 框架 | Framework | 8.5 | — | `composer create-project` |
| [Yii3](framework/yii3.md) | 框架 | Framework | 8.5 | — | `composer create-project` |
| [WordPress](cms/wordpress.md) | CMS | Traditional | 8.5 | MySQL | WP-CLI |
| [Drupal](cms/drupal.md) | CMS | Framework | 8.4 | MySQL | `drush site:install` |
| [Craft CMS](cms/craft.md) | CMS | Framework | 8.5 | MySQL | `craft install` |
| [October CMS](cms/october.md) | CMS | Framework + mirror | 8.4 | MySQL | `october:migrate` + mirror |
| [Magento](ecommerce/magento.md) | 电商 | Framework | 8.4 | MySQL + OpenSearch | `bin/magento setup:install` |
| [OpenCart](ecommerce/opencart.md) | 电商 | Traditional | 8.4 | MySQL | CLI 安装器 |

## 每个配方的共同点

### 基于已发布的 OxPHP 镜像构建

OxPHP 以 `ghcr.io/oxphp/oxphp` 形式发布了一个开箱即用的 PHP 运行时（默认 PHP 8.5；PHP 8.4 变体以 `ghcr.io/oxphp/oxphp:<ver>-php8.4-alpine<X>` 发布）。已发布的镜像已经包含 `oxphp` 二进制文件、`libphp.so`、OxPHP SAPI 扩展、PHP CLI，以及对 Composer 友好的工具链。配方以两种方式之一对其进行扩展：

1. **将 OxPHP 复制进 `php:*-zts-alpine` 基础镜像**（Laravel、Symfony、Yii3、Craft、Magento、OpenCart、Drupal、October 采用此方式）。多阶段构建从四个阶段组装出一个 `dev` 镜像：

   ```dockerfile
   FROM php:8.4-zts-alpine3.23 AS php-base       # 你的应用的 PHP 扩展
   FROM composer:2            AS composer        # Composer 二进制文件
   FROM ghcr.io/oxphp/oxphp:0.10.0-php8.4-alpine3.23 AS oxphp   # OxPHP 产物
   FROM php-base AS dev                          # 最终镜像
   # ... 复制 oxphp 二进制文件、bridge 库和 SAPI 扩展：
   COPY --from=oxphp /usr/local/bin/oxphp              /usr/local/bin/oxphp
   COPY --from=oxphp /usr/local/lib/liboxphp_bridge.so /usr/local/lib/
   COPY --from=oxphp /usr/local/lib/php/extensions/    /tmp/oxphp-ext/
   RUN cp /tmp/oxphp-ext/*/oxphp_sapi.so "$(php -r 'echo ini_get("extension_dir");')/" \
       && echo "extension=oxphp_sapi.so" > /usr/local/etc/php/conf.d/oxphp-ext.ini
   ```

2. **直接扩展 OxPHP 运行时**（WordPress 采用此方式）。一个 builder 阶段针对匹配的 `php:*-zts-alpine` 编译扩展，并将 `.so` 文件放入 OxPHP 镜像中。

无论采用哪种方式，**PHP ABI 都必须匹配**。OxPHP 镜像的 `libphp.so` 和 `oxphp_sapi.so` 是针对某个 PHP 版本编译的（例如 8.4 → `no-debug-zts-20240924`）；`php-base`／builder 阶段必须使用相同的 `php:<X.Y>-zts-alpine<Z>`，这样你编译的扩展才能保持 ABI 兼容。混用版本会导致 `oxphp_sapi.so` 拒绝加载，或在启动时破坏 musl TLS。

规范的多阶段模板参见 [Docker 指南](../getting-started/docker.md)，仓库中的 [`examples/dockerfile/`](../../../examples/dockerfile/) 提供了可直接复制的版本。

### 有意识地选择 PHP 版本

默认的 `ghcr.io/oxphp/oxphp:<ver>` 镜像是 **PHP 8.5**。这对现代框架（Laravel、Symfony、Yii3、Craft）来说没有问题。较旧或较保守的代码库 —— Magento、OpenCart、Drupal、October CMS —— 则通过 `…-php8.4-alpine…` 标签固定到 **PHP 8.4**，因为它们的组件栈早于 8.5，在 8.5 上会触发弃用警告。每个配方都会说明它使用哪个版本以及原因。

### 根据应用的形态选择路由模式

OxPHP 的[路由模式](../features/routing.md)与应用的目录布局直接对应：

- **Framework mode**（`ENTRY_FILE=index.php`）—— 在 `public/`（或 `web/`、`pub/`）目录下有一个 front controller；已存在的静态文件从磁盘提供，其余一切都被分派到 `index.php`。适用于 Laravel、Symfony、Yii3、Craft、Magento、Drupal、October。
- **Traditional mode**（无 `ENTRY_FILE`）—— 多个物理 PHP 入口（例如 `index.php` 加上一个 `admin/` 目录）作为真实文件提供。适用于 WordPress 和 OpenCart。

### 在同一个容器中安装

`dev` 镜像携带了 PHP CLI 和 Composer（以及视情况而定的 `drush`、`wp`、`bin/magento`、`php yii`、`php craft`、`php artisan`），因此每条安装和维护命令都在运行中的容器内执行 —— 无需独立的工具链：

```bash
docker compose exec app php artisan migrate      # Laravel
docker compose exec app vendor/bin/drush cr      # Drupal
docker compose run  --rm app composer install    # 任意
```

### 处处适用的安全默认值

OxPHP 免费为你提供了若干保护，而这些在 nginx + PHP-FPM 下需要显式配置：

- [点路径阻断](../security/dot-path-blocking.md) —— `.env`、`.git/`、`.htaccess` 以及任何其他点段路径无需配置即返回 `404`。这正是为什么从一个同时包含 `.env` 的目录运行应用不会泄露它。
- [PHP 执行拒绝清单](../security/php-deny.md)（`PHP_DENY_PATHS`）—— 由 traditional-mode 配方采用：OpenCart 阻断 `system/` 和 `install/` 脚本；WordPress 阻断 `wp-content/uploads/` 下的 `.php` 执行。（在 framework mode 下为无操作，因为那里从不会直接执行任意 `.php`。）
- [符号链接允许路径](../security/symlink-allow-paths.md)（`SYMLINK_ALLOW_PATHS`）—— 由 October CMS 采用，使 OxPHP 跟随 `october:mirror public` 生成的资源符号链接，同时在其他任何地方仍阻断符号链接逃逸。

## 这些配方

页面按应用类型分组，与目录布局对应：

```
examples/
├── framework/   # Laravel、Symfony、Yii3
├── cms/         # WordPress、Drupal、Craft、October
└── ecommerce/   # Magento、OpenCart
```

### 框架 —— `framework/`

- [Laravel](framework/laravel.md) —— 典范的 framework-mode 应用
- [Symfony](framework/symfony.md) —— 最小骨架，无数据库
- [Yii3](framework/yii3.md) —— 最精简者；仅核心扩展

### CMS —— `cms/`

- [WordPress](cms/wordpress.md) —— traditional mode、运行时扩展构建、WP-CLI 边车
- [Drupal](cms/drupal.md) —— framework mode、PDO + `drush`
- [Craft CMS](cms/craft.md) —— framework mode、控制台驱动安装
- [October CMS](cms/october.md) —— framework mode，带 `public/` 镜像和 `SYMLINK_ALLOW_PATHS`

### 电商 —— `ecommerce/`

- [Magento](ecommerce/magento.md) —— 最重量级：OpenSearch、PHP 8.4、静态资源版本符号链接
- [OpenCart](ecommerce/opencart.md) —— traditional mode，带两个 front controller 和 `PHP_DENY_PATHS`
