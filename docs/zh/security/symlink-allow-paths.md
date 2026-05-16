---
title: 符号链接允许路径
description: DOCUMENT_ROOT 外符号链接目标的显式允许列表 — 适用于 Laravel 风格的 storage:link、共享资源卷、多租户上传目录。
---

# 符号链接允许路径

默认情况下，OxPHP 拒绝任何解析结果落在规范 `DOCUMENT_ROOT` 之外的请求。`DOCUMENT_ROOT` 内指向外部目录的符号链接会返回 404，路由日志会输出 `Blocked request: resolved path escapes document root`。

这是合理的默认值 — 它阻止目录穿越、symlink-swap TOCTOU 攻击，以及意外泄露上一层的配置文件或密钥。但它同样会拦截框架多年来使用的合法模式：Laravel 的 `php artisan storage:link`、Symfony 资源包、挂载到多个容器的共享上传卷。

`SYMLINK_ALLOW_PATHS` 是显式的 opt-in 机制：你列出 `DOCUMENT_ROOT` 内符号链接允许指向的文件系统路径。不在列表中的目标依然按 404 严格处理。

## 配置

```bash
# 逗号分隔的绝对路径
SYMLINK_ALLOW_PATHS=/var/www/storage,/opt/shared/assets

# 相对路径基于 DOCUMENT_ROOT 解析
SYMLINK_ALLOW_PATHS=../storage,../shared/uploads

# 允许混合使用
SYMLINK_ALLOW_PATHS=/opt/shared/cdn,../storage/app/public
```

未设置（默认）时，任何符号链接都不能离开 `DOCUMENT_ROOT`。

## Laravel 示例

```bash
DOCUMENT_ROOT=/app/public
SYMLINK_ALLOW_PATHS=../storage/app/public
```

`php artisan storage:link` 会在项目内创建 `public/storage -> ../storage/app/public`。`/storage/<file>` 类 URL 经符号链接解析至 `/app/storage/app/public/<file>` — 这是允许列表授权的路径。应用代码无需修改。

## 工作原理

启动时，每个条目都会被解析：

- **绝对路径条目** — 先检查黑名单（见下文），再通过 `realpath(3)`（`std::fs::canonicalize`）。若 `realpath` 失败（目标不存在），服务器拒绝启动。
- **相对路径条目** — 与规范 `DOCUMENT_ROOT` 拼接后再执行 `realpath`。

得到的规范路径作为允许列表存储。重复项静默去重。

请求时，路由层将解析后的文件路径规范化，并验证其满足以下条件之一：

1. 位于 `DOCUMENT_ROOT` 之内，或
2. 恰好等于允许列表中的某个条目（文件目标），或
3. 以允许列表中的某个条目加 `/` 开头（目录目标）。

同样的检查在静态文件服务路径中作为 TOCTOU 防护再执行一次 — 在路由缓存之后、任何 read 系统调用之前。

## 黑名单

少量路径永远不能出现在 `SYMLINK_ALLOW_PATHS` 中 — 否则拼写错误或误解会显著扩大攻击面。任何条目解析到黑名单时服务器拒绝启动。

**精确匹配禁止：**

```
/   /etc   /proc   /sys   /dev   /var   /home   /tmp   /root   /usr   /srv
```

**作为前缀禁止**（条目位于这些目录下）：

```
/etc   /proc   /sys   /dev   /tmp   /root   /usr
```

注意 `/var`、`/home` 和 `/srv` 只在精确匹配时被拒 — 裸 `/srv` 会被拒绝，但 `/srv/myapp/storage` 是被允许的，与 `/var/www/storage` 或 `/home/<任何>/...` 同理。每个条目检查两次：一次针对管理员输入的原始路径（防止 macOS 风格的 `/etc -> /private/etc` 通过 `realpath` 洗掉黑名单），一次针对规范化后的形式（对符号链接目标逃逸的纵深防御）。

黑名单本身是硬编码的 — 没有可扩展它的环境变量。默认值是抓住"输错路径"级错误的保守最低限度；需要更严格策略的管理员应在外层叠加（文件系统权限、容器挂载限制、AppArmor/SELinux 策略）。

## 启动失败模式

| 配置错误 | 结果 |
|---|---|
| 条目目标在磁盘上不存在 | 服务器拒绝启动，错误信息包含条目名和 `canonicalize` |
| 条目命中黑名单（原始或规范化路径） | 服务器拒绝启动，错误信息包含条目名和黑名单规则 |
| 空值或仅空白的环境变量 | 视为未设置 — 应用严格默认行为 |
| 重复条目 | 规范化后静默去重 |
| 启动时尚不存在的符号链接 | 允许列表已注册但暂未生效，直到符号链接出现；启动时不要求符号链接存在 |

## 安全说明

- 允许列表是 opt-in — 变量未设置时保留"绝不外逃"的安全默认值
- 条目在启动时规范化，所以条目路径中的 `..` 和中间符号链接会在存储前折叠
- 运行时规范化关闭了 symlink-swap TOCTOU 窗口 — 验证过的路径就是实际读取的路径
- 文件目标按精确匹配；目录目标按目录前缀匹配。将单个文件 `/opt/shared/license.key` 加入允许列表不会隐式授予其同级文件的访问权
- 路径验证结果按 URL 缓存（每个唯一 URL 一次 `realpath`，直到淘汰），所以运行时开销被均摊

## 参见

- [PHP 拒绝路径](php-deny.md) — 按 URI 通配模式阻止 PHP 执行；与符号链接策略正交
- [点路径阻止](dot-path-blocking.md) — 拒绝 `.well-known` 风格穿越和 dotfile 泄露
- [受信任代理](trusted-proxies.md) — `X-Forwarded-*` 头部的独立信任边界
- [配置参考](../operations/configuration.md) — 所有环境变量
