---
title: PHP 执行拒绝名单
description: 阻止在指定 URI 路径执行 PHP，加固遗留应用，防御上传的 PHP shell 和意外的脚本执行。
---

# PHP 执行拒绝名单

`PHP_DENY_PATHS` 阻止匹配指定 glob 模式的 `.php` 文件被直接执行。它针对遗留 PHP 应用中常见的一类漏洞：攻击者将 PHP 文件上传到可写的公共目录（`/uploads`、`/cache`、图片缩放的临时目录），再通过直接 URI 访问该文件来获得代码执行能力。

匹配检查在**任何磁盘 I/O 之前**进行，因此被拒路径无论文件是否存在都返回相同响应——攻击者无法将拒绝名单当作 existence oracle 用来枚举上传目录中的真实文件名。

## 适用范围

适用于直接文件映射模式——即 URI 直接解析到磁盘上 `.php` 文件的模式：

| 路由模式 | `PHP_DENY_PATHS` 是否生效 |
|---|---|
| Traditional（未设置 `ENTRY_FILE`） | 是 |
| SPA（`ENTRY_FILE=index.html`） | 是 —— SPA 会直接执行磁盘上已存在的 `.php` 文件，因此拒绝名单适用 |
| Framework（`ENTRY_FILE=index.php`） | 否 —— 警告并忽略 |
| Worker（`WORKER_MODE_ENABLED=true`） | 否 —— 警告并忽略 |

在 Framework 模式下，每个请求都被重写到前端控制器，任意 `.php` 文件从不被直接执行；拒绝名单只会破坏以 `.php` 结尾的应用路由。在 Worker 模式下，每个非静态请求都被分发到 worker 脚本，拒绝名单无可拒绝之物。在这两种模式下设置 `PHP_DENY_PATHS` 会在启动时输出 warning 并禁用该检查。

拒绝名单也覆盖*间接*到达的脚本：当 `uploads/**` 在名单中时，对 `/uploads/` 的请求若经目录索引查找解析到 `uploads/index.php`，同样会被拒绝——模式匹配的对象包括解析后的脚本路径，而不仅是请求 URI。对于此类拒绝，`OXPHP_DENIED_PATH` 携带去除尾部斜杠的净化请求 URI（`/uploads/` 报告为 `/uploads`）。

## 配置

```bash
# 逗号分隔的 glob 模式列表
PHP_DENY_PATHS="/uploads/**,/cache/**,/tmp/**"

# 命中时返回什么（默认：404）
PHP_DENY_FALLBACK="403"
```

对 `/uploads/shell.php` 的请求现在会返回 403，并且不会触碰磁盘。对 `/uploads/image.png` 的请求则照常处理——拒绝名单仅影响 `.php` 执行，不影响静态文件提供。

## 模式语法

模式针对已规范化的 URI（路径已解析 `..` 段并解码百分号编码）匹配，使用 `globset` 语法。每个模式开头的 `/` 是可选的——`/uploads/**` 与 `uploads/**` 等价。

| 模式 | 匹配 | 不匹配 |
|---|---|---|
| `/uploads/**` | `/uploads/x.php`、`/uploads/a/b/c.php`、`/uploads/shell.php/extra` | `/uploads.php`、`/public/uploads/x.php` |
| `/files/*.php` | `/files/x.php` | `/files/sub/x.php`（单个 `*` 不跨越 `/`） |
| `/admin/legacy.php` | `/admin/legacy.php` | `/admin/legacy.php/x`（PATH_INFO 未覆盖——见下文） |
| `/admin/legacy.php{,/**}` | `/admin/legacy.php`、`/admin/legacy.php/x` | `/admin/other.php` |
| `/**/wp-config.php` | `/wp-config.php`、`/site/wp-config.php` | `/wp-config.txt` |

多个模式通过 OR 组合——只要请求匹配任一模式即视为命中。

### 单个文件 vs 目录

两种都可以。`/uploads/**` 锁定整个子树；`/admin/legacy.php` 只锁定某个具体脚本。如果希望同时覆盖单个入口点及其 PATH_INFO 调用（`/admin/legacy.php/foo`），使用花括号形式：`/admin/legacy.php{,/**}`。

### 大小写敏感

匹配**区分大小写**。在大小写不敏感的文件系统上（macOS 默认 HFS+/APFS、Windows 默认 NTFS、启用 `casefold` 的 ext4），对 `/uploads/Shell.PHP` 的请求会绕过 `/uploads/**/*.php` 模式。在这类文件系统上建议使用宽泛的目录模式 `/uploads/**`（捕获任意扩展名），或在写入时将上传文件名规范化为小写。

## Fallback 模式

`PHP_DENY_FALLBACK` 控制命中时返回的内容。

### HTTP 状态码

`400`–`599` 之间的任意值（默认 `404`）。可与 `ERROR_PAGES_DIR` 配合提供自定义 HTML 主体：

```bash
PHP_DENY_PATHS="/uploads/**"
PHP_DENY_FALLBACK="403"
ERROR_PAGES_DIR="/var/www/errors"  # 提供 errors/403.html
```

### PHP 脚本

以 `/` 开头、指向 `DOCUMENT_ROOT` 内 PHP 回退脚本的 URI 路径：

```bash
PHP_DENY_PATHS="/uploads/**"
PHP_DENY_FALLBACK="/_security/denied.php"
```

该脚本在启动时严格校验——必须存在、规范化路径必须位于 `DOCUMENT_ROOT` 内，且脚本自身不得命中 `PHP_DENY_PATHS`（防止循环；否则启动中止）。脚本运行时会在 `$_SERVER` 中收到两个额外键，标识原始请求：

| `$_SERVER` 键 | 含义 |
|---|---|
| `OXPHP_DENIED_PATH` | 原始的规范化 URI，带开头 `/`（与 `PATH_INFO` 形式相同） |
| `OXPHP_DENIED_PATTERN` | 匹配到的 glob 模式 |

`OXPHP_DENIED_PATTERN` 存储时**不带**开头 `/`（按 glob 规范化），而 `OXPHP_DENIED_PATH` 保留请求 URI 的 `/`。若要将路径与模式比较，请先 `ltrim($_SERVER['OXPHP_DENIED_PATH'], '/')`，使两者形式一致。

honeypot 示例：

```php
<?php
// /_security/denied.php —— 替代任何被命中的 .php 请求执行。
error_log(sprintf(
    "PHP execution denied: path=%s pattern=%s ip=%s ua=%s",
    $_SERVER['OXPHP_DENIED_PATH'] ?? '',
    $_SERVER['OXPHP_DENIED_PATTERN'] ?? '',
    $_SERVER['REMOTE_ADDR'] ?? '',
    $_SERVER['HTTP_USER_AGENT'] ?? '-',
));

http_response_code(404);
echo "Not Found";
```

这样你可以为每个请求决定如何应答（攻击者返回 404、已登录管理员返回 403、把扫描器重定向到 sinkhole），不必受限于单一静态状态码。

## 无 existence oracle

状态码和脚本两种 fallback 都不触碰文件系统返回。对 `/uploads/never-uploaded.php` 与 `/uploads/actually-on-disk.php` 的请求产生完全相同的响应——既无时间差异，也无主体差异。扫描上传 shell 的攻击者无法利用拒绝名单来枚举真实存在的文件名。

解析路径检查是唯一的例外：它必须在路由解析之后运行，因此其拒绝依赖于文件是否存在。只有当 `uploads/index.php` 真实存在于磁盘上时，`/uploads/` 才会被拒绝；同样，对于 `/uploads/*.php` 这样的单星号模式，PATH_INFO 请求 `/uploads/shell.php/x` 只有在 `uploads/shell.php` 存在时才被拒绝（完整 URI 不匹配该模式——解析出的脚本才匹配）。攻击者用来探测的直接 URI 检查依然没有 existence oracle。

## 可观测性

| 指标 | 描述 |
|---|---|
| `oxphp_php_deny_total` | 每次被拒请求加一的计数器 |

每次拒绝同时会输出一条 `tracing::info` 日志：

```
PHP execution denied by PHP_DENY_PATHS path=uploads/shell.php pattern=uploads/**
```

访问日志只记录最终状态码（`PHP_DENY_FALLBACK` 的值，或 fallback 脚本中 `http_response_code()` 的设置）——访问日志层面无法区分被拒请求与普通请求。若要归因流量峰值，请结合指标或结构化日志。

## 性能

匹配是一次 `globset::GlobSet` 查找，通常对 URI 字节做一次 SIMD 扫描。命中还会绕过路由缓存（被拒 URI 来自攻击者喷射，基数实际无界；缓存它们会让攻击者把合法条目从 LRU 中挤出）。预热之后命中和未命中路径都不再分配。

## 限制

诚实列举此功能**不会**做的事：

- **字面文件模式的 PATH_INFO 绕过**：模式 `/admin/legacy.php` 不匹配 `/admin/legacy.php/extra`。使用 `/admin/legacy.php{,/**}` 同时覆盖两种情况，或改用目录模式。
- **匹配区分大小写**（见上文[大小写敏感](#大小写敏感)）。
- **不支持正则**：仅支持 glob——锚定式，运算符包括 `*`/`**`/`?`/`[abc]`/`{a,b}`。`(a|b)` 这样的写法请改用多个逗号分隔的模式。
- **不影响 `include` / `require` / `eval`**：拒绝名单仅约束*通过 URI 直接*执行。即便配置了拒绝名单，含 `include $_GET['page']` 的脆弱脚本仍可加载服务器可读的任意 PHP。

## 已弃用的别名

旧变量 `PHP_DENY_DIRS` 作为已弃用别名仍被接受，启动时会输出 warning：

```
WARN PHP_DENY_DIRS is deprecated, use PHP_DENY_PATHS instead — the alias will be removed in a future release
```

当两者都设置时，`PHP_DENY_PATHS` 胜出，`PHP_DENY_DIRS` 被报告为忽略。两个值不会合并。

## 参见

- [路由](../features/routing.md) —— 路由模式与路径安全
- [错误页面](../features/error-pages.md) —— 状态码 fallback 响应的自定义 HTML 主体
- [配置参考](../operations/configuration.md) —— 完整的环境变量列表
- [指标](../operations/metrics.md) —— `oxphp_php_deny_total` 等相关指标
