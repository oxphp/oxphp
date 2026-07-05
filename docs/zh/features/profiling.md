---
title: PHP 代码性能分析
description: 最详尽的实用指南——从首次启动到生产环境使用。触发器、PHP SDK、属性、导出格式，speedscope/xhgui/pprof 集成，指标与故障排查。
---

# 在 OxPHP 中对 PHP 代码进行性能分析

OxPHP 内置按请求粒度的性能分析器。与 xdebug 或独立扩展不同，它运行在服务器进程内部，
无需重启 PHP，在关闭时也几乎没有开销（`mode=Off` 分支在过滤器缓存查询之前就直接返回）。

本文档是一份**实用指南**：从零配置，到定位生产环境慢请求、阅读 flamegraph、
进行优化前/后对比。

---

## 1. TL;DR — 60 秒启动

```yaml
# compose.yml
services:
  app:
    image: ghcr.io/oxphp/oxphp:0.9.0
    environment:
      INTERNAL_ADDR: 0.0.0.0:9090
      PROFILER_ENABLED: "true"
      PROFILER_AUTH_TOKEN: "dev-secret"
      PROFILER_OUTPUT_FORMATS: "xhprof,speedscope,collapsed"
    volumes:
      - ./www:/var/www/html
      - profiles:/tmp/oxphp-profiles
    ports:
      - "80:80"
      - "9090:9090"

volumes:
  profiles:
```

```bash
# 1. 带上性能分析触发头发起请求。
curl -H "X-OxPHP-Profile: dev-secret" http://localhost/slow-endpoint

# 2. 查看已捕获的运行列表。
curl -H "Authorization: Bearer dev-secret" http://localhost:9090/__profiler/runs \
  | jq '.runs[0]'

# 3. 在 speedscope 中打开火焰图（浏览器内）。
open "http://localhost:9090/__profiler/runs/<run_id>/speedscope"
```

完成。下面详细说明背后的机制及生产环境用法。

---

## 2. 性能分析器的能力

- **捕获每一次 PHP 函数调用**——通过 Zend Observer API，无需字节码补丁，
  无需改动用户代码。
- **构建 Span 树**——记录 wall-time、CPU 时间、入口/出口内存、属性和事件。
- **同时导出 4 种格式**：`xhprof.json`、`speedscope.json`、
  `pprof`（protobuf + gzip）、`collapsed`（供 `flamegraph.pl` 使用）。
- **持久化运行数据**：内存 LRU 缓存 + 磁盘文件 + 可选 HTTP 推送
  （xhgui 或自建收集器）。
- **暴露 8 个内部 HTTP 路由**——绑定在 `INTERNAL_ADDR`，用于浏览和下载配置文件。
- **输出 Prometheus 指标**——runs/source、采集的 spans、写入字节数、
  丢弃、推送失败。
- **无需重启**——单次请求即可通过触发器激活。

---

## 3. 内部工作原理

```
 ┌─ 请求 ────────────────────────────────────────────────────────────┐
 │  1. Tokio 线程：ProfilerRequestHandler 检查触发器                   │
 │     （header/cookie/query/sample_rate，常量时间比较）               │
 │                                                                    │
 │  2. 决策写入 PluginRequestActions → 经 SAPI 通道发送至 worker       │
 │                                                                    │
 │  3. Worker 在 RINIT 之前设置 ProfilingMode = ProfileAll             │
 │     并对每个函数的 begin/end 注册 Observer handlers                │
 │                                                                    │
 │  4. 每次 PHP 函数调用 → C hook → bridge 缓冲区 → Rust               │
 │     SpanTree（span_id = 单调 BE 计数器；函数名在                    │
 │     thread-local interner 中 intern — 无额外分配）                 │
 │                                                                    │
 │  5. 响应后：ProfilerCompleteHandler 获取 Arc<SpanTree>，            │
 │     触发 4 个导出器、写入 LRU 缓存，spawn 磁盘写入和 HTTP 推送任务  │
 │     （信号量限制扇出）                                              │
 └────────────────────────────────────────────────────────────────────┘
```

### 单次请求的三种模式

| 模式 | 何时激活 | 捕获范围 |
|------|--------------------|-------------------|
| `Off` | 默认。没有插件请求性能分析。 | 不捕获。零开销。 |
| `ApmOnly` | `plugin-apm` 启用但 Profiler 触发器未命中。 | 仅 APM 的显式钩子：`#[Trace]`、PDO/cURL emitter、`oxphp_trace_*()`。 |
| `ProfileAll` | Profiler 触发器命中（或调用了 `OxPHP\Profile\start()`）。 | 经由 Observer API 捕获的**每一次** PHP 函数调用 + APM 收集的所有内容。 |

`ProfileAll` 会「覆盖」`ApmOnly`：当两个插件都启用且触发器命中时，
共用同一个 `Arc<SpanTree>`——不会重复采集。

---

## 4. 安装与构建

`plugin-profiler` 插件属于**默认 cargo feature**。标准的
`docker compose build` 已经包含它。

关闭：

```dockerfile
ARG OXPHP_WITH_PROFILER=0
# 或
ARG CARGO_FEATURES="plugin-apm,plugin-otel"  # 不包含 plugin-profiler
```

检查插件是否已编译：

```bash
docker compose exec app cat /proc/self/maps | grep -i profiler
# 或：oxphp --list-plugins（若可用）
```

---

## 5. 激活方式——4 种触发器

检查顺序（优先级）：**header → cookie → query → sample_rate**。
任一命中即激活 `ProfileAll`。令牌以**常量时间**比较
（`subtle::ConstantTimeEq`）。

### 5.1. HTTP 请求头（开发与脚本）

```bash
curl -H "X-OxPHP-Profile: dev-secret" https://app.local/checkout
```

适合 CI 基准、Postman 集合、curl 脚本。

### 5.2. Cookie（浏览器调试）

通过 DevTools / 扩展在浏览器中设置 cookie：

```
OXPROF=dev-secret; Domain=app.local; Path=/
```

只要 cookie 存在，每个请求都会被分析。删除 cookie 即停用。
适合走完用户场景（打开商品 → 加入购物车 → 下单）以获得**一组**配置文件。

### 5.3. Query 参数（链接分享）

```
https://app.local/admin/report?__oxprof=dev-secret
```

最粗暴的方式，但方便发给同事「打开这个，就能复现 bug」。
注意：参数会出现在访问日志和 Referer 中——不要使用生产令牌。

### 5.4. 随机采样（生产环境）

```bash
PROFILER_SAMPLE_RATE=0.001   # ≈ 1000 个请求中采 1 个
```

**无需令牌**。在生产环境打开即可积累真实流量下的统计数据。
推荐起始值 `0.0005..0.002`；再高会带来可察觉的开销，尤其是当
`PROFILER_INTERNAL=true` 时。

### 5.5. 从采样中排除路径

`PROFILER_SAMPLE_RATE` 会随机采样**所有**请求——包括污染数据的框架自身流量。
Symfony 的 Web Debug Toolbar 轮询 `/_wdt/{token}` 并链接到 `/_profiler/{token}`；
Laravel Debugbar 和 Telescope 也类似。用 `PROFILER_EXCLUDE_PATHS` 把它们排除在采样之外：

```bash
PROFILER_EXCLUDE_PATHS=/_profiler,/_profiler/**,/_wdt/**
```

逗号分隔的 glob 模式，语法与 `PHP_DENY_PATHS` 相同：`*` 不跨越 `/`，`**` 跨越，
开头的 `/` 可选。只有同时列出两者，才能既覆盖裸路径**又**覆盖其子树——
`/_profiler/**` 匹配 `/_profiler/x` 但不匹配裸 `/_profiler`，因此有上面的双模式写法。
模式按原样匹配请求路径——不做 percent 解码或 `..` 规范化——因此请填写框架实际使用的字面路径。

> **排除仅影响自动采样。** 携带显式触发器（`x-oxphp-profile` 请求头、`OXPROF`
> Cookie 或 `__oxprof` 查询参数）的请求**始终**会被分析，即使在被排除的路径上。
> 这样你可以有意分析 `/_profiler` 本身，同时把它排除在后台采样之外。

---

## 6. 配置——完整参考

| 变量 | 默认值 | 说明 |
|---|---|---|
| `PROFILER_ENABLED` | `false` | 主开关。`true` → 插件加载。 |
| `PROFILER_AUTH_TOKEN` | *(未设)* | 触发器密钥及 `/__profiler/*` 路由的 Bearer 令牌。空字符串 = 「无需令牌」（任意非空触发值都通过）。**不要把令牌提交到仓库。** |
| `PROFILER_SAMPLE_RATE` | `0.0` | `[0.0; 1.0]`。随机采样率。 |
| `PROFILER_EXCLUDE_PATHS` | *(未设)* | 逗号分隔的 glob 模式（`PHP_DENY_PATHS` 语法），从 `PROFILER_SAMPLE_RATE` 中排除。显式触发器仍会分析它们。示例：`/_profiler,/_profiler/**,/_wdt/**`。 |
| `PROFILER_INTERNAL` | `false` | 观测**内部** C 函数（`strlen`、`json_encode` 等）。覆盖全面，但**额外开销 2–5 倍**。仅在需要时定点开启。 |
| `PROFILER_MAX_SPANS` | `50000` | 单请求 Span 树大小硬上限。超出后后续 Span 被标记为 `truncated`，不再写入。 |
| `PROFILER_MAX_DEPTH` | `256` | 栈深度硬上限。 |
| `PROFILER_OUTPUT_DIR` | `/tmp/oxphp-profiles` | 绝对路径。必须对 `www-data` 可写。 |
| `PROFILER_OUTPUT_FORMATS` | `xhprof,speedscope` | `xhprof`、`speedscope`、`pprof`、`collapsed` 的 CSV 子集。 |
| `PROFILER_RETENTION_COUNT` | `100` | 保留的运行数（磁盘与 LRU 共享）。后台每 5 秒裁剪一次。 |
| `PROFILER_DISK_MAX_PER_SEC` | `10` | 保护磁盘的令牌桶。超出部分丢弃，`oxphp_profiler_disk_drops_total` 递增。 |
| `PROFILER_EXPORT_URL` | *(未设)* | 将每个运行 POST 推送到的 URL（xhgui、自建收集器）。 |
| `PROFILER_EXPORT_FORMAT` | `xhprof` | HTTP 推送使用的四种格式之一。 |
| `PROFILER_EXPORT_AUTH_TOKEN` | *(未设)* | 推送目标的 Bearer 令牌。 |
| `PROFILER_EXPORT_XHGUI` | `auto` | 强制 xhgui 信封模式。Auto：URL 路径以 `/run/import` 结尾（xhgui 的规范端点；host/query 中的字符串不匹配——非标准路径请设为 `true`）。 |
| `PROFILER_EXPORT_BUGGREGATOR` | `auto` | 强制使用 Buggregator 信封。Auto：URL 路径以 `/api/profiler/store` 结尾。此信封始终发送 xhprof，因此 `PROFILER_EXPORT_FORMAT` 对它无效（非 xhprof 取值仅告警，不致命）。与 `PROFILER_EXPORT_XHGUI` 互斥（同时启用会在启动时报错）。 |
| `PROFILER_EXPORT_APP_NAME` | *（未设置）* | Buggregator 的 `app_name`——profile 归属的项目。 |
| `PROFILER_EXPORT_TAGS` | *（未设置）* | Buggregator 的 `tags`，格式 `key=value,key2=value2`，用于过滤。非法项（非 `key=value`）、空键或重复键会在启动时报错。 |

### 生产环境示例配置

```yaml
environment:
  PROFILER_ENABLED: "true"
  PROFILER_AUTH_TOKEN: "${PROFILER_TOKEN_FROM_VAULT}"
  PROFILER_SAMPLE_RATE: "0.001"             # ~0.1% 流量
  PROFILER_INTERNAL: "false"
  PROFILER_OUTPUT_DIR: /var/lib/oxphp/profiles
  PROFILER_OUTPUT_FORMATS: "xhprof,collapsed"
  PROFILER_RETENTION_COUNT: "500"
  PROFILER_DISK_MAX_PER_SEC: "20"
  PROFILER_EXPORT_URL: "http://xhgui.monitoring.svc.cluster.local/run/import"
  PROFILER_EXPORT_FORMAT: "xhprof"
```

---

## 7. PHP SDK——七个函数

所有函数都在 `OxPHP\Profile` 命名空间下。它们始终可以安全调用：
若当前请求未激活性能分析，修改器都是安全的 no-op，`is_active()` 返回 `false`。

### 7.1. 精准开关一段代码

```php
use function OxPHP\Profile\{start, stop, is_active};

function heavy_report(): array
{
    start();                       // 在请求内激活 ProfileAll
    $result = build_report();      // 会进入 Span 树
    stop();                        // 停止捕获
    return $result;
}
```

`start()` 幂等。`stop()` 同样——连续调用两次也安全。

> ⚠️ 在请求中途调用 `start()` 会**重置**当前 Span 树（见
> `php_sdk.rs` 中的 `PROFILING_CONTEXT.reset()`）。这遵循规范：
> 每个请求 mode 只能设置**一次**——要么由 RINIT 时的触发器，
> 要么由首次 `start()` 调用。

### 7.2. 暂停与恢复

```php
use function OxPHP\Profile\{pause, resume};

pause();
noisy_helper_we_dont_care_about();  // 不会进入树
resume();
```

与 `stop()` 不同，pause/resume 语义上表达「临时」。
内部是同一个标志，但区分可以提升代码可读性。

### 7.3. 标记点——mark()

```php
use function OxPHP\Profile\mark;

mark('cache_miss');
mark('got_auth_token', ['user_id' => (string) $user->id]);
```

在**当前最顶层开放 Span** 上附加 `SpanEventKind::Mark` 事件。
没有打开的 Span 时 no-op。适合在长函数中打临时时间戳，
或标记 if/else 分支。

### 7.4. 数值指标——metric()

```php
use function OxPHP\Profile\metric;

$rows = $pdo->query('SELECT ...')->fetchAll();
metric('db.rows', (float) count($rows));
metric('payload.kb', strlen($body) / 1024.0);
```

向当前 Span 的**属性**追加 `metric.<name>=<value>`。
与 `mark()` 不同，这只是键值对（无时间戳）。
在 speedscope / xhgui 的 Span 属性面板中显示。

### 7.5. 状态判断——is_active()

```php
if (OxPHP\Profile\is_active()) {
    // 可以承担昂贵的 debug-dump——
    // 反正这个请求正在被分析
    error_log(json_encode($debug_state));
}
```

两次 TLS 读取，无 FFI。可以在热点路径中安全调用。

---

## 8. 属性（PHP 8）——声明式控制

七个属性分为两类：**observer 过滤器**在 Span 创建**之前**运行；
**装饰器**在 Span 结束**之后**运行。

| 属性 | 类别 | 效果 |
|---|---|---|
| `#[Profile]` | 过滤器 | 强制将该函数纳入树（即使通用规则会排除它）。 |
| `#[Exclude]` | 过滤器 | 跳过该函数；其子节点重新归属到最近的被包含祖先。 |
| `#[Sample(rate: 0.1)]` | 过滤器 | 仅保留部分调用（`rate ∈ [0.0; 1.0]`）。概率式，无锁。 |
| `#[Tag(key, value)]` | 过滤器 | 给 Span 附加标签。该属性 repeatable——多个 `#[Tag]` 会累积。 |
| `#[Mark(label?)]` | 装饰器 | 在函数入口触发 `Mark` 事件。 |
| `#[SlowThreshold(ms)]` | 装饰器 | 当 wall-time ≥ `ms` 时触发 `Slow` 事件并设置状态。 |
| `#[MemoryThreshold(kb)]` | 装饰器 | 当净分配内存 ≥ `kb` 时触发 `MemorySpike` 并设置状态。 |

### 类与方法的组合

```php
use OxPHP\Profile\{Tag, Profile, Exclude};

#[Tag(key: 'layer', value: 'domain')]
#[Profile]                                    // 整个类始终被分析
class OrderService
{
    #[Tag(key: 'op', value: 'create')]
    public function create(array $data): Order { /* ... */ }

    #[Exclude]                                 // 尽管类上有 #[Profile]，此方法仍被排除
    public function debug_dump(): void { /* ... */ }

    public function find(int $id): ?Order { /* ... */ }   // 继承 #[Profile] 和 #[Tag(layer)]
}
```

- 类级属性**传播**到所有方法。
- 方法级属性**追加**到类级属性（tag 累积）。
- 方法的 `#[Exclude]` **覆盖**类级的 `#[Profile]`。

### 慢函数阈值

```php
use OxPHP\Profile\SlowThreshold;

#[SlowThreshold(ms: 250)]
function render_dashboard(User $u): string
{
    // 若执行 ≥ 250 毫秒，Span 上会追加 Slow 事件且
    // status_code=2（error）。在 xhgui / speedscope 中立刻可见。
}
```

### 内存阈值

```php
use OxPHP\Profile\MemoryThreshold;

#[MemoryThreshold(kb: 512)]
function import_csv(string $path): int
{
    // 若函数执行期间净分配 ≥ 512 KB，
    // 触发 MemorySpike 事件 + status=error
}
```

### 对单个函数进行采样

```php
use OxPHP\Profile\Sample;

#[Sample(rate: 0.01)]
function log_event(string $evt, array $ctx): void
{
    // 约 1% 的调用进入树；其余完全跳过——
    // 不创建 Span，也不创建其子节点。适合单次请求被
    // 调用数百万次的函数。
}
```

> **过滤器 vs 装饰器？** 如果函数调用**极其频繁**且想降低捕获开销——
> 使用 `#[Sample]` 或 `#[Exclude]`（Span 创建前生效）。
> 如果需要在阈值超限时添加事件——使用 `#[SlowThreshold]` /
> `#[MemoryThreshold]`（它们查看已采集完成的 Span）。

---

## 9. Span 捕获的数据

```ruby
FinishedSpan {
  span_id         # Arc<str>，兼容 W3C
  parent_span_id  # Arc<str>
  trace_id        # Arc<str>，与 APM 共享
  name            # PHP 函数/方法的完整限定名
  start_ns        # wall-clock，从 profiler epoch 起的纳秒
  end_ns
  cpu_ns          # CLOCK_THREAD_CPUTIME_ID（平台不支持时为 0）
  memory_start    # 入口处的 zend_memory_usage(0)
  memory_end      # 出口处的 zend_memory_usage(0)
  attributes      # Vec<(Arc<str>, Arc<str>)> — 来自 #[Tag]、metric()、APM SQL/HTTP
  events          # Vec<SpanEvent { ts, kind, label, attrs }>
  status_code     # 0 = unset、1 = ok、2 = error
  status_message
  leaked          # 当 Span 在 finalize 中被强制关闭（PHP 抛出穿越 observer）时为 true
}
```

**事件类型**（`SpanEvent::kind`）：

| Kind | 触发方 |
|---|---|
| `Mark` | `mark()`、`metric()`、`#[Mark]` |
| `Slow` | `#[SlowThreshold]` |
| `MemorySpike` | `#[MemoryThreshold]` |
| `Sql` | APM 钩子（PDO、mysqli） |
| `Http` | APM 钩子（cURL、HTTP streams） |
| `Exception` | APM 异常处理器 |
| `Alloc` | （保留给堆采样） |
| `Other` | 兜底 |

---

## 10. 导出格式——何时使用哪种

文件位于 `PROFILER_OUTPUT_DIR`，文件名 `<run_id>.<ext>`，其中
`run_id = <ts_ms>-<req_id_prefix>-<rand4>`（例如
`1713600000000-a1b2c3d4-0f5e`）。

### 10.1. speedscope（🏆 交互式分析默认选择）

扩展名：`.speedscope.json`

- 浏览器内火焰图，支持缩放、搜索、CPU / 时间 / 内存切换。
- 零配置——直接打开 [speedscope.app](https://www.speedscope.app/)。
- OxPHP 在 `/__profiler/runs/{id}/speedscope` 返回 302 重定向，
  携带 `profileURL=…` 参数，speedscope.app 会直接从你的服务器加载 profile。

```bash
# macOS 终端 Ctrl-click / Linux xdg-open
open "http://localhost:9090/__profiler/runs/<run_id>/speedscope"
```

### 10.2. xhprof（用于 xhgui——时间线 + 历史对比）

扩展名：`.xhprof.json`

- 与 xhgui 兼容（按 URL 搜索、趋势、两个运行的 diff）。
- 非常适合**生产累积**：运行一个挨着应用的 xhgui 容器，
  设置 `PROFILER_EXPORT_URL=http://xhgui/run/import`——
  UI 中会累积历史。
- 现成的 docker-compose：`tests/compose.xhgui.yml`。

### 10.3. pprof（Google pprof 工具、Grafana pprof 插件、Pyroscope）

扩展名：`.pprof`（protobuf + gzip，`fast` 级别，zlib backend）

```bash
# 保存并打开
curl -H "Authorization: Bearer dev-secret" \
  http://localhost:9090/__profiler/runs/<run_id>.pprof > profile.pprof

go tool pprof -http=:8080 profile.pprof
# 或
pyroscope-cli adhoc --input profile.pprof
```

### 10.4. collapsed（Brendan Gregg 的 flamegraph.pl）

扩展名：`.collapsed`

- 文本格式 `func;child;grandchild <count>`。
- SVG 火焰图的事实标准输入。
- 三种指标变体：wall-time、CPU、内存。OxPHP 写入 `.collapsed`
  （wall）；内部路径也会产生 `.collapsed.cpu` 和 `.collapsed.mem`
  （参见 `tests/fixtures/profiler_exports/`）。

```bash
curl -H "Authorization: Bearer dev-secret" \
  http://localhost:9090/__profiler/runs/<run_id>.collapsed \
  | flamegraph.pl --title "Checkout $run_id" > flame.svg
```

### 10.5. Buggregator（本地调试服务器）

[Buggregator](https://buggregator.dev) 是一个单文件调试服务器，功能之一是将 xhprof profile 渲染为按项目分组的火焰图。`xhprof` 推送直接发往它的 `POST /api/profiler/store` 端点——无需 xhprof PHP 扩展或客户端库，数据由 OxPHP 的原生 profiler 生成。

```yaml
services:
  buggregator:
    image: ghcr.io/buggregator/server:latest
    ports: ["8000:8000"]

  app:
    image: ghcr.io/oxphp/oxphp:latest
    environment:
      PROFILER_ENABLED: "true"
      PROFILER_SAMPLE_RATE: "0.01"
      PROFILER_EXPORT_URL: "http://buggregator:8000/api/profiler/store"
      PROFILER_EXPORT_FORMAT: "xhprof"
      PROFILER_EXPORT_APP_NAME: "checkout"          # 按项目分组
      PROFILER_EXPORT_TAGS: "env=staging,region=eu" # UI 中可过滤
```

路径以 `/api/profiler/store` 结尾的 URL 会自动选择 Buggregator 信封（`PROFILER_EXPORT_BUGGREGATOR: "true"` 可为自定义 URL 强制启用；`"false"` 可关闭）。此信封始终发送 xhprof，因此 `PROFILER_EXPORT_FORMAT` 对它无效（非 xhprof 取值在启动时仅告警，不致命——profiler 绝不会因导出配置而使服务器崩溃）。`app_name` 和 `tags` 驱动 Buggregator 的项目分组与过滤；不设置时 profile 仍会渲染，但不分组。`hostname` 取自 `$HOSTNAME`，该变量未设置时回退到 `gethostname(2)` 系统调用。

---

## 11. 存储与清理

```
/tmp/oxphp-profiles/
├── index.json                                # NDJSON——每行一条记录
├── 1713600000000-a1b2c3d4-0f5e.xhprof.json
├── 1713600000000-a1b2c3d4-0f5e.speedscope.json
└── 1713600001234-b2c3d4e5-4a2b.xhprof.json
```

### `index.json` 条目 schema

```json
{
  "run_id": "1713600000000-a1b2c3d4-0f5e",
  "request_id": "a1b2c3d4e5f67890",
  "trace_id": "0af7651916cd43dd8448eb211c80319c",
  "timestamp_ms": 1713600000000,
  "duration_ms": 123,
  "method": "GET",
  "url": "/checkout",
  "status": 200,
  "user_agent": "Mozilla/5.0 …",
  "client_ip": "10.0.0.42",
  "source": "Header",                 // Header | Cookie | Query | SampleRate
  "span_count": 4821,
  "event_count": 7,
  "error_count": 0,
  "leaked_count": 0,
  "truncated": false,                 // true — 超出 PROFILER_MAX_SPANS
  "oxphp_version": "0.9.0",
  "formats": ["xhprof.json", "speedscope.json"]
}
```

`index.json` 由 `/__profiler/runs` 路由解析，按新到旧排序，
通过 `?limit=N&offset=M` 分页。

### Retention（保留策略）

- 后台任务每 5 秒删除超出 `PROFILER_RETENTION_COUNT` 的条目
  （原子 `rename` → `index.json`）。
- `index.json` 之外的孤儿文件（服务器崩溃遗留）被 sweep 清理。
- `PROFILER_DISK_MAX_PER_SEC` 令牌桶保护磁盘：超速部分不写入，
  `oxphp_profiler_disk_drops_total` 递增。

---

## 12. 内部 HTTP 路由

当 `INTERNAL_ADDR=0.0.0.0:9090` 时，插件在 `/__profiler/`
前缀下注册 8 个端点。配置了令牌时，全部要求
`Authorization: Bearer <PROFILER_AUTH_TOKEN>`。比较为**常量时间**。

| 路由 | 方法 | 用途 |
|---|---|---|
| `/__profiler/` | GET | 端点索引的 HTML landing page。 |
| `/__profiler/runs` | GET | 运行 JSON 数组。支持 `?limit=N&offset=M`。 |
| `/__profiler/runs/{id}` | GET | 单个运行的 JSON 元数据。 |
| `/__profiler/runs/{id}.{format}` | GET | 原始 profile 字节。`format` ∈ `xhprof.json`、`speedscope.json`、`pprof`、`collapsed`。 |
| `/__profiler/runs/{id}/speedscope` | GET | 302 → speedscope.app，携带 `profileURL=…`。 |
| `/__profiler/runs/{id}` | DELETE | 删除所有格式文件 + 索引条目（返回 204）。 |
| `/__profiler/config` | GET | 插件当前配置（令牌被屏蔽）。 |
| `/__profiler/stats` | GET | JSON 计数器快照。 |

### 示例脚本

```bash
# 最近 20 次运行中按耗时排序的 Top-5
curl -s -H "Authorization: Bearer $TOK" \
     "http://localhost:9090/__profiler/runs?limit=20" \
  | jq '.runs | sort_by(.duration_ms) | reverse | .[:5]'

# 指定 URL 的全部 profile
curl -s -H "Authorization: Bearer $TOK" \
     "http://localhost:9090/__profiler/runs?limit=500" \
  | jq '.runs[] | select(.url == "/checkout")'

# 删除 1 小时之前的全部 runs（与插件 retention 独立）
NOW=$(date +%s%3N)
CUTOFF=$((NOW - 3600000))
curl -s -H "Authorization: Bearer $TOK" \
     "http://localhost:9090/__profiler/runs?limit=1000" \
  | jq -r --arg c "$CUTOFF" '.runs[] | select(.timestamp_ms < ($c|tonumber)) | .run_id' \
  | xargs -I{} curl -X DELETE -H "Authorization: Bearer $TOK" \
       "http://localhost:9090/__profiler/runs/{}"
```

---

## 13. HTTP 推送 + xhgui

将每个运行发送到远程收集器：

```yaml
environment:
  PROFILER_EXPORT_URL: "http://xhgui/run/import"
  PROFILER_EXPORT_FORMAT: "xhprof"
  PROFILER_EXPORT_AUTH_TOKEN: "shared-secret"   # 可选
```

- xhgui 信封自动识别：**URL 路径以 `/run/import` 结尾**（xhgui 的规范端点）。
  host/query 中的 `xhgui` 字符串不匹配——此类 URL 请用
  `PROFILER_EXPORT_XHGUI=true|false` 强制设定。
- 重试策略：3 次指数退避 `100/200/400 毫秒`，总预算 5 秒墙钟时间。
  请求体跨重试以 `bytes::Bytes` 共享（零重试分配）。
- 失败递增 `oxphp_profiler_http_push_failures_total`。

### 完整 demo 栈

```bash
docker compose -f tests/compose.xhgui.yml up -d
# 应用：:80，xhgui：:8142（UI），:27017（mongo）
```

E2E 冒烟测试：`tests/php/profiler/test_xhgui_import.php`。

---

## 14. Prometheus 指标

暴露在 `/metrics`：

```
oxphp_profiler_runs_total{source="header"|"cookie"|"query"|"sample"}
oxphp_profiler_spans_collected_total
oxphp_profiler_bytes_written_total{format="xhprof"|"speedscope"|"pprof"|"collapsed"}
oxphp_profiler_disk_drops_total
oxphp_profiler_http_push_failures_total
oxphp_profiler_truncated_total
oxphp_profiler_in_memory_runs
```

入门级 Prometheus 告警：

```yaml
- alert: ProfilerDiskDrops
  expr: rate(oxphp_profiler_disk_drops_total[5m]) > 0
  annotations:
    summary: "Profiler 正在丢弃磁盘运行——检查 PROFILER_DISK_MAX_PER_SEC"

- alert: ProfilerPushFailing
  expr: rate(oxphp_profiler_http_push_failures_total[5m]) > 0
  annotations:
    summary: "xhgui/收集器不可达"

- alert: ProfilerTruncatingTrees
  expr: rate(oxphp_profiler_truncated_total[5m]) > 0
  annotations:
    summary: "请求超出 PROFILER_MAX_SPANS——提升上限或排查原因"
```

---

## 15. 实战流程——逐步指南

### 15.1. 定位慢 endpoint

1. 在生产启用 `PROFILER_SAMPLE_RATE=0.001`，等待数据积累。
2. 按 `duration_ms` 排序：
   ```bash
   curl -s -H "Authorization: Bearer $TOK" \
        "http://INT_ADDR/__profiler/runs?limit=500" \
     | jq '.runs | sort_by(.duration_ms) | reverse | .[:10]
            | map({run_id, url, duration_ms, span_count})'
   ```
3. 在 speedscope 中打开 Top-1：`.../__profiler/runs/<id>/speedscope`。
4. 在 speedscope 中启用 **Left Heavy** 模式——立刻看到累计时间最长的函数。
5. 点击最宽的矩形——得到 file:line 和子节点列表。

### 15.2. 验证「优化前/后」假设

1. 变更前运行基准：
   ```bash
   for i in $(seq 1 20); do
     curl -s -H "X-OxPHP-Profile: dev-secret" http://localhost/api/report > /dev/null
   done
   curl -s -H "Authorization: Bearer dev-secret" \
        "http://localhost:9090/__profiler/runs?limit=20" \
     | jq '.runs | map(.duration_ms) | add / length' > /tmp/p50_before.txt
   ```
2. 应用变更，重建，重跑。对比中位数。
3. 深度 diff 可下载两个 xhprof profile 上传到 xhgui——它内置 diff 视图。

### 15.3. 内存泄漏排查

1. 发送「越跑越大」的请求：
   ```bash
   curl -H "X-OxPHP-Profile: dev-secret" http://localhost/import?file=big.csv
   ```
2. 在 speedscope 中打开，切换到 **memory metric**
   （通过 `.collapsed.mem` 或 speedscope 的内存视图）。
3. 为可疑函数加上 `#[MemoryThreshold(kb: 1024)]`——下一次运行会
   得到明显的 `MemorySpike` 事件。
4. 使用 `metric('mem.after', memory_get_usage())` 进行定点打标。

### 15.4. 持续监控关键路径

```php
#[Profile]
#[SlowThreshold(ms: 500)]
public function chargeCard(PaymentRequest $r): PaymentResult
{
    // 始终被捕获；若变慢会显式打 Slow 标记
}
```

在 Grafana 中添加来自 `oxphp_profiler_runs_total{source="sample"}`
的面板；并针对 `index.json` 中 `duration_ms` 的异常值（通过基于日志
的 metric 或 sidecar 导出器）设置告警。

### 15.5. 通过链接复现 bug

同事反馈「/admin/report 对我返回 500」。回复：

```
https://app.local/admin/report?__oxprof=<一次性-令牌>
```

访问后——`/__profiler/runs?limit=5`，打开 profile，
就能看到异常发生的确切位置（`status_code=2` + `Exception` 事件）。

---

## 16. 与 APM（`plugin-apm`、OpenTelemetry）的协作

- 两个插件**共享**同一个 `Arc<SpanTree>`。不会重复采集。
- Profiler 无触发 + APM 启用 → `mode=ApmOnly`。树中只有显式
  标记的 Span（`#[Trace]`、APM 的 SQL/HTTP 钩子）。
- Profiler 触发命中 → `mode=ProfileAll`。树中包含**全部**
  内容加上 APM 的标注。
- 无论如何，APM 都**只**将显式 Span 导出到 OTLP（Jaeger/Tempo
  每个 trace 上限约 10k 个 Span）。完整细节请用 `/__profiler/runs/<id>`。

---

## 17. 最佳实践

1. **永远不要把 `PROFILER_AUTH_TOKEN` 提交到仓库**。从 Vault /
   Docker secrets / Kubernetes secrets 读取。
2. **生产只用 `SAMPLE_RATE`**。Header/Cookie/Query 是开发工具。
   如需生产按需分析，用单独的、每日轮换的令牌。
3. **不要全局开启 `PROFILER_INTERNAL=true`**。2–5 倍开销会把生产变实验室。
   隔离环境中定点使用。
4. **让 `PROFILER_RETENTION_COUNT` 贴合实际**——单次运行体积可从几百 KB
   （小请求）到几 MB（大树）不等。500 runs × 2 MB = 1 GB。
   按需规划磁盘。
5. **对噪声 helper 使用 `#[Exclude]`**（日志、i18n、autoloader）——
   树更易读，语义不损失。
6. **将 profile 与 trace 关联**：`trace_id` 共享。在 Grafana / Kibana
   的 trace 视图中链接到 `/__profiler/runs/<id>`。
7. **使用 Git 友好的标识符**。本版本 `span_id` 是确定性 big-endian
   单调计数器。两个保存的 profile diff 非常干净。
8. **APM + Profiler 是免费组合**。可以同时开启；树共享，
   开销只随 APM 实际采集的覆盖增加。

---

## 18. 故障排查

### 「没有 profile 出现」

1. 插件是否已编译？`docker compose build` 默认包含。
   确认没有传 `--build-arg OXPHP_WITH_PROFILER=0` 或自定义的
   不含 `plugin-profiler` 的 `CARGO_FEATURES`。
2. `PROFILER_ENABLED=true`？
3. 触发器是否真的匹配 `PROFILER_AUTH_TOKEN`？
   - 检查环境变量中是否夹带了 `\n`。
   - query 方式下，URL 是否正确编码？
4. 服务器是否收到你的请求？查看访问日志。

### 401 来自 `/__profiler/runs`

头里的 Bearer 令牌与 `PROFILER_AUTH_TOKEN` 不匹配。常见陷阱：
`echo "secret" > secret.txt` 会追加 `\n`。改用 `printf` 或从 env 注入。

### 「xhgui 没有新 runs」

1. 检查可达性：
   ```bash
   docker compose exec app curl -v $PROFILER_EXPORT_URL
   ```
2. 查看 `oxphp_profiler_http_push_failures_total`。
3. 查看日志：每次失败都会打印带 `run_id` 和 HTTP 状态的 `tracing::warn!`。

### 磁盘上没有文件

- `PROFILER_OUTPUT_DIR` 是否为**绝对路径**？相对路径会被忽略。
- 对 `www-data` 可写吗？
  ```bash
  docker compose exec app ls -la /tmp/oxphp-profiles
  ```
- `PROFILER_DISK_MAX_PER_SEC` 是否过低？查看
  `oxphp_profiler_disk_drops_total`。

### 生产开销过大

- `PROFILER_INTERNAL=false`（这是默认）。
- `PROFILER_SAMPLE_RATE` 保持在合理范围（0.0005..0.002）。
- `PROFILER_MAX_SPANS` 合理——超出后树被截断但 capture 继续。
  对极大请求更建议用定点 `start()`/`stop()` 包住关注区域。

### `index.json` 中 `truncated=true`

请求超过了 `PROFILER_MAX_SPANS`（默认 50,000）。选项：
1. 提高上限（牺牲内存换细节）。
2. 对被调用数万次的函数加 `#[Exclude]` / `#[Sample(rate: 0.01)]`。
3. 只对可疑区域用 `start()`/`stop()` 包裹。

---

## 19. 命令速查

```bash
# 为单个请求激活
curl -H "X-OxPHP-Profile: $TOK" http://localhost/endpoint

# 列出运行，按耗时 Top-10
curl -sH "Authorization: Bearer $TOK" http://localhost:9090/__profiler/runs \
  | jq '.runs | sort_by(.duration_ms) | reverse | .[:10]'

# 在 speedscope 中打开
open "http://localhost:9090/__profiler/runs/$RUN_ID/speedscope"

# 下载为 xhprof 供 xhgui 导入
curl -sH "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID.xhprof.json > run.xhprof.json

# 下载为 pprof 并打开
curl -sH "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID.pprof > run.pprof
go tool pprof -http=:8080 run.pprof

# flamegraph.pl
curl -sH "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID.collapsed \
  | flamegraph.pl > flame.svg

# 删除一个运行
curl -X DELETE -H "Authorization: Bearer $TOK" \
  http://localhost:9090/__profiler/runs/$RUN_ID

# 指标
curl -s http://localhost:9090/metrics | grep oxphp_profiler_

# 当前插件配置（安全——令牌已屏蔽）
curl -sH "Authorization: Bearer $TOK" http://localhost:9090/__profiler/config | jq
```

---

## 20. 实战示例（完整代码）

下面是可直接放进 `www/public/` 并用 curl 访问的 PHP 场景。

### 20.1. 简单控制器——手动控制

```php
<?php
// www/public/report.php
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, mark, metric, is_active};

function fetch_rows(PDO $db, int $user_id): array
{
    $stmt = $db->prepare('SELECT * FROM orders WHERE user_id = ? LIMIT 1000');
    $stmt->execute([$user_id]);
    return $stmt->fetchAll(PDO::FETCH_ASSOC);
}

function render_report(array $rows): string
{
    $sum = array_sum(array_column($rows, 'amount'));
    return json_encode(['count' => count($rows), 'total' => $sum]);
}

$db = new PDO('mysql:host=db;dbname=app', 'app', 'secret');
$user_id = (int) ($_GET['user_id'] ?? 1);

// 即使没有外部触发器，也显式分析重活块
start();

mark('report.begin', ['user_id' => (string) $user_id]);

$rows = fetch_rows($db, $user_id);
metric('db.rows', (float) count($rows));

$body = render_report($rows);
metric('response.bytes', (float) strlen($body));

mark('report.done');
stop();

header('Content-Type: application/json');
echo $body;

// 可选——告诉前端本次请求被分析过：
if (is_active()) {
    header('X-Profiled: 1');
}
```

调用：

```bash
curl -H "X-OxPHP-Profile: dev-secret" 'http://localhost/report.php?user_id=42'
```

### 20.2. 使用属性的服务类

```php
<?php
// www/lib/OrderService.php
declare(strict_types=1);

use OxPHP\Profile\{Profile, Tag, Exclude, Sample, SlowThreshold, MemoryThreshold};

#[Profile]
#[Tag(key: 'layer', value: 'domain')]
#[Tag(key: 'svc',   value: 'orders')]
final class OrderService
{
    public function __construct(
        private readonly PDO $db,
        private readonly Mailer $mailer,
    ) {}

    #[SlowThreshold(ms: 250)]
    #[Tag(key: 'op', value: 'create')]
    public function create(array $payload): int
    {
        $this->db->beginTransaction();
        try {
            $id = $this->insertOrder($payload);
            $this->insertLines($id, $payload['items']);
            $this->db->commit();
            $this->mailer->sendReceipt($id);
            return $id;
        } catch (\Throwable $e) {
            $this->db->rollBack();
            throw $e;
        }
    }

    #[MemoryThreshold(kb: 2048)]
    #[Tag(key: 'op', value: 'export')]
    public function exportCsv(int $user_id): string
    {
        $stmt = $this->db->prepare('SELECT * FROM orders WHERE user_id = ?');
        $stmt->execute([$user_id]);

        $buf = fopen('php://temp', 'r+');
        fputcsv($buf, ['id', 'created_at', 'total']);
        while ($row = $stmt->fetch(PDO::FETCH_ASSOC)) {
            fputcsv($buf, [$row['id'], $row['created_at'], $row['total']]);
        }
        rewind($buf);
        return stream_get_contents($buf);
    }

    // 简单的 getter，不让它占据树的空间
    #[Exclude]
    public function find(int $id): ?array
    {
        $stmt = $this->db->prepare('SELECT * FROM orders WHERE id = ?');
        $stmt->execute([$id]);
        return $stmt->fetch(PDO::FETCH_ASSOC) ?: null;
    }

    // 极高频的审计——采样以避免树被撑爆
    #[Sample(rate: 0.05)]
    private function audit(string $event, array $ctx): void
    {
        $this->db->prepare('INSERT INTO audit (event, ctx) VALUES (?, ?)')
                 ->execute([$event, json_encode($ctx)]);
    }

    private function insertOrder(array $p): int { /* ... */ return 0; }
    private function insertLines(int $id, array $items): void { /* ... */ }
}
```

### 20.3. 批处理任务——只分析第一次迭代

```php
<?php
// www/bin/import.php
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, pause, resume, mark};

$files = glob('/data/incoming/*.csv');
$i = 0;

foreach ($files as $path) {
    if ($i === 0) {
        start();                       // 只完整分析第一个文件
        mark('batch.begin', ['path' => $path]);
    } else {
        pause();                       // 其余——capture no-op
    }

    import_one($path);

    if ($i === 0) {
        mark('batch.first_done');
        stop();
    }
    $i++;
}

function import_one(string $path): void { /* ... */ }
```

### 20.4. 对比两种实现——带 profile 的微基准

```php
<?php
// www/public/bench.php —— 朴素 vs 流式实现对比
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, mark, metric};

function naive_sum(string $path): int
{
    $rows = array_map('str_getcsv', file($path));           // 整文件读入内存
    return array_sum(array_column($rows, 1));
}

function streaming_sum(string $path): int
{
    $h = fopen($path, 'r');
    $total = 0;
    while (($row = fgetcsv($h)) !== false) {
        $total += (int) ($row[1] ?? 0);
    }
    fclose($h);
    return $total;
}

$path = '/data/big.csv';
$which = $_GET['impl'] ?? 'naive';

start();
mark('bench.begin', ['impl' => $which]);
$t0 = hrtime(true);

$result = $which === 'naive' ? naive_sum($path) : streaming_sum($path);

$elapsed_ms = (hrtime(true) - $t0) / 1e6;
metric('bench.elapsed_ms', $elapsed_ms);
metric('bench.result',     (float) $result);
mark('bench.done');
stop();

echo json_encode(['impl' => $which, 'elapsed_ms' => $elapsed_ms, 'result' => $result]);
```

工作流：

```bash
# 朴素实现
curl -H "X-OxPHP-Profile: dev-secret" "http://localhost/bench.php?impl=naive"

# 流式实现
curl -H "X-OxPHP-Profile: dev-secret" "http://localhost/bench.php?impl=streaming"

# 在 xhgui 中 diff（最近的两个 xhprof 运行）
curl -sH "Authorization: Bearer dev-secret" \
     "http://localhost:9090/__profiler/runs?limit=2" | jq '.runs[] | .run_id'
```

### 20.5. 生产代码中的条件式分析

```php
<?php
// 典型场景：某个可疑函数仅对特定用户偶发变慢。
declare(strict_types=1);

use function OxPHP\Profile\{start, stop, is_active};

function charge(User $user, Money $amount): PaymentResult
{
    // 审计：若当前请求在被分析，
    // 就对第三方调用打开更详尽日志。
    $verbose = is_active();

    $gateway = new StripeClient(verbose: $verbose);
    return $gateway->charge($user->id, $amount);
}

function oncall_path(Order $order): void
{
    // 仅对 VIP 用户启用 profile，无需外部触发器。
    if ($order->user->tier === 'vip') {
        start();
    }
    process($order);
    if ($order->user->tier === 'vip') {
        stop();
    }
}
```

### 20.6. 自我分析的集成测试

```php
<?php
// tests/php/profile_smoke.php
declare(strict_types=1);
require __DIR__ . '/test_helper.php';

use function OxPHP\Profile\{start, stop, mark, is_active};

$t = new TestCase('profile_smoke', 'my-app');

// 手动启用 profiler（测试 SDK 无需触发器）
$t->assertFalse('initially not active', is_active());
start();
$t->assertTrue('active after start', is_active());

mark('test.midpoint');

// 模拟计算
$sum = 0;
for ($i = 0; $i < 100_000; $i++) { $sum += $i; }

stop();
$t->assertFalse('inactive after stop', is_active());
$t->assertSame('computation OK', $sum, 4999950000);

$t->done();
```

### 20.7. 通过 Postman 系列请求定位热点

场景：「/api/search 偶发变慢，并不稳定」。

```javascript
// Postman Pre-request Script
pm.request.headers.add({
    key: 'X-OxPHP-Profile',
    value: pm.environment.get('PROFILE_TOKEN')
});
```

跑完 100 次后，用 `jq` 一行命令找 Top 异常：

```bash
curl -sH "Authorization: Bearer $TOK" \
     "http://int.app.local:9090/__profiler/runs?limit=200" \
  | jq -r '.runs
          | map(select(.url | startswith("/api/search")))
          | sort_by(-.duration_ms)
          | .[:5]
          | map("\(.duration_ms)ms  \(.run_id)  \(.url)")
          | .[]'
```

### 20.8. 自定义装饰器 + Profiler

自定义 `#[ProfileDb]`——记录行数并自动调用 `metric('db.rows', …)`：

```php
<?php
use OxPHP\Decorator\{AttributeInterface, Context};
use function OxPHP\Profile\metric;

#[Attribute(Attribute::TARGET_METHOD)]
class ProfileDb implements AttributeInterface
{
    public function before(Context $ctx): void {}

    public function after(Context $ctx): void
    {
        $result = $ctx->returnValue;
        if (is_array($result)) {
            metric('db.rows', (float) count($result));
        } elseif ($result instanceof PDOStatement) {
            metric('db.rows', (float) $result->rowCount());
        }
    }
}

oxphp_register_decorator(ProfileDb::class);

class UserRepository
{
    #[ProfileDb]
    public function findAll(): array { /* ... */ return []; }
}
```

装饰器 + profiler 的组合开箱即用：`metric()` 会自动附加到
Observer API 当前正在观测的函数对应的 Span。

---

## 21. 参考

- 代码内规范：`src/profiling/mod.rs`、`src/plugins/ox_profiler/`
- Bridge (C)：`ext/bridge/oxphp_bridge.c`、`ext/oxphp_sapi.c`
- PHP 测试：`tests/php/profiler/`
- 格式 fixture：`tests/fixtures/profiler_exports/`
- xhgui demo：`tests/compose.xhgui.yml`
- speedscope：<https://www.speedscope.app/>
- xhgui：<https://github.com/perftools/xhgui>
- Google pprof：<https://github.com/google/pprof>
- flamegraph.pl：<https://github.com/brendangregg/FlameGraph>
