---
title: Shared\* 可观测性
description: OxPHP\Shared\* 原语的内省端点、Prometheus 指标与诊断手册——查看在线注册表条目、追踪可达性图、并对饱和度告警。
---

# Shared\* 可观测性

每个 `OxPHP\Shared\*` 实例都是运行时已为引用计数和容量跟踪的注册表条目。该跟踪以 `/__ox_shared/*` 下的 JSON 内省和 `oxphp_shared_*` 下的 Prometheus 指标对运维人员暴露。本文是参考手册，也是现场指南。

## 启用

可观测性依托于[内部服务器](../features/internal-server.md)。设置 `INTERNAL_ADDR` 即可启动它：

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

JSON 端点和 `/metrics` 都会在该地址可达。无需额外配置。

你可以独立关闭其中任一项：

| 环境变量                         | 默认值  | 作用                                                         |
|---------------------------------|---------|--------------------------------------------------------------|
| `SHARED_INTROSPECTION_ENABLED`  | `true`  | 开关 `/__ox_shared/*` JSON API。                             |
| `SHARED_INTROSPECTION_PREVIEW_ENABLED` | `true` | 开关 `/preview`（值形态预览可能泄漏数据）。            |
| `SHARED_METRICS_ENABLED`        | `true`  | 开关 `oxphp_shared_*` Prometheus 指标。                      |

在面向不可信租户的部署中关闭内省；指标仅为聚合值，开启是安全的。

## 内省端点

所有响应均为 `Content-Type: application/json; charset=utf-8`。查询参数采用标准 URL 编码。

### `GET /__ox_shared/summary`

顶层快照：按类型聚合的计数、内存、操作速率，以及与配置上限的饱和度。

```json
{
  "total_entries": 127,
  "total_bytes": 2_481_664,
  "by_type": {
    "Counter": { "count": 48, "bytes": 3_072,   "ops": 1_402_391 },
    "Map":     { "count": 12, "bytes": 1_638_400, "ops":    48_201 },
    "Pool":    { "count":  4, "bytes":   16_384, "ops":    67_014 }
  },
  "limits":   { "max_entries": 100_000, "max_bytes": 1073741824, "soft_ratio": 0.7 },
  "saturation": { "entries": 0.00127, "bytes": 0.00231 },
  "diagnostics": {
    "lock_diagnostics_level": "warn",
    "cycle_detect_depth": 16,
    "poison_strict": false
  }
}
```

在仪表板和 cron 告警中使用 `summary`。一次抓取即可获得每类型健康度和容量余量。

### `GET /__ox_shared/entries?limit=N`

列出存活条目（被 `limit` 截断，默认 100，最大 500）。每个条目一行：

```json
{
  "items": [
    { "id": 42, "type": "Map",    "refcount": 2, "ops":  1820, "mem_bytes": 204_800, "age_sec": 612 },
    { "id": 43, "type": "Counter", "refcount": 3, "ops": 48_014, "mem_bytes":     64, "age_sec": 612 }
  ],
  "next_cursor": null,
  "total_matching": 127
}
```

`refcount` 是外部保留计数——即多少 PHP 包装和嵌套 Shared 条目持有它。当你期望某条目应可被 GC 而它却没有时，正是要查的字段。

### `GET /__ox_shared/entry?id=N`

某个条目的类型特定细节：

```json
{
  "id": 42,
  "type": "Map",
  "refcount": 2,
  "ops": 1820,
  "mem_bytes": 204_800,
  "age_sec": 612,
  "type_specific": {
    "key_count": 1_240,
    "max_entries": 50_000,
    "saturation": 0.0248,
    "sample_keys": ["tenant:acme", "tenant:beta", "..."]
  }
}
```

`type_specific` 因类型而异——Pool 暴露 `{ size, in_use, idle, waiting, idle_by_thread, max_size }`，Channel 暴露 `{ capacity, pending, closed, senders_blocked, receivers_blocked }`，Counter 暴露 `{ value }`，等等。

### `GET /__ox_shared/preview?id=N`

标量值和小数组的形态预览。字符串值按 `SHARED_PREVIEW_STRING_LIMIT`（默认 256 字节）截断；数组展示前 `SHARED_PREVIEW_ARRAY_LIMIT` 个条目（默认 20）。受 `SHARED_INTROSPECTION_PREVIEW_ENABLED` 控制。

```json
{ "id": 42, "type": "Counter", "preview": "1420" }
```

开发期使用 `preview`；生产期当值可能包含用户数据时关闭它。

### `GET /__ox_shared/types`

枚举 v1 类型目录——对需要 tag → class 映射的生成式工具有用：

```json
{
  "types": [
    { "tag": 10, "name": "Counter", "php_class": "OxPHP\\Shared\\Counter" },
    { "tag": 11, "name": "Flag",    "php_class": "OxPHP\\Shared\\Flag" },
    { "tag": 12, "name": "Once",    "php_class": "OxPHP\\Shared\\Once" },
    { "tag": 20, "name": "Map",     "php_class": "OxPHP\\Shared\\Map" },
    { "tag": 30, "name": "Mutex",   "php_class": "OxPHP\\Shared\\Mutex" },
    { "tag": 31, "name": "Channel", "php_class": "OxPHP\\Shared\\Channel" },
    { "tag": 50, "name": "Pool",    "php_class": "OxPHP\\Shared\\Pool" }
  ]
}
```

### `GET /__ox_shared/graph?id=N[&depth=D][&edges=E]`

从 `id=N` 起对出向 `Shareable` 引用做 BFS 遍历。返回可达子图的节点与边。默认值：`depth=16`、`edges=500`。命中遍历器预算时响应中 `truncated: true`。

```json
{
  "root": 42,
  "nodes": [
    { "id": 42, "type": "Map",     "refcount": 2, "mem_bytes": 204_800 },
    { "id": 51, "type": "Counter", "refcount": 1, "mem_bytes":     64 }
  ],
  "edges": [
    { "from": 42, "to": 51, "key": "hits" }
  ],
  "truncated": false
}
```

在 `CycleException` 之后用 `graph` 查看遍历器走过的可达路径，或在排查「这个 Counter 为什么不被 GC」时——图会展示每一个对它持有保留的父级。

## Prometheus 指标

所有指标在 `GET /metrics` 与核心服务器指标一起暴露。

### 注册表级

| 指标                                            | 类型     | 标签            | 描述                                                |
|------------------------------------------------|---------|-----------------|----------------------------------------------------|
| `oxphp_shared_objects_total`                   | gauge   | `type`          | 每类型存活条目数。                                  |
| `oxphp_shared_operations_total`                | counter | `type`          | 分发到每类型的累计操作数。                          |
| `oxphp_shared_bytes`                           | gauge   | `type`          | 每类型近似字节数（与 `mallinfo` 相比 ±30%）。       |
| `oxphp_shared_total_bytes`                     | gauge   | —               | 跨类型求和。                                        |
| `oxphp_shared_capacity_saturation`             | gauge   | `kind`          | `entries` 和 `bytes` 相对其上限的占比。             |
| `oxphp_shared_deadlock_detected_total`         | counter | —               | 检测到的跨线程 wait-for 循环数。                    |

### Channel

| 指标                                              | 类型    | 标签           |
|--------------------------------------------------|---------|---------------|
| `oxphp_shared_channel_count`                     | gauge   | `channel_id`  |
| `oxphp_shared_channel_pending` *(已弃用)*        | gauge   | `channel_id`  |
| `oxphp_shared_channel_senders_blocked`           | gauge   | `channel_id`  |
| `oxphp_shared_channel_receivers_blocked`         | gauge   | `channel_id`  |
| `oxphp_shared_channel_items_sent_total`          | counter | `channel_id`  |
| `oxphp_shared_channel_items_dropped_total`       | counter | `channel_id`  |

`oxphp_shared_channel_pending` 是 `oxphp_shared_channel_count` 的旧
拼写；在 deprecation 周期内两条序列携带相同的值，并在未来某个发布
移除别名时分离。新仪表盘请配置 `_count`。

### Map

| 指标                                     | 类型    | 标签       |
|-----------------------------------------|---------|------------|
| `oxphp_shared_map_entries`              | gauge   | `map_id`   |
| `oxphp_shared_map_max_entries`          | gauge   | `map_id`   |
| `oxphp_shared_map_saturation`           | gauge   | `map_id`   |

### Pool

| 指标                                        | 类型       | 标签                                  |
|---------------------------------------------|-----------|---------------------------------------|
| `oxphp_shared_pool_count`                   | gauge     | `pool_id`                             |
| `oxphp_shared_pool_size` *(已弃用)*         | gauge     | `pool_id`                             |
| `oxphp_shared_pool_in_use`                  | gauge     | `pool_id`                             |
| `oxphp_shared_pool_idle`                    | gauge     | `pool_id`                             |
| `oxphp_shared_pool_waiting`                 | gauge     | `pool_id`                             |
| `oxphp_shared_pool_acquire_total`           | counter   | `pool_id`                             |
| `oxphp_shared_pool_evicted_total`           | counter   | `pool_id`, `reason`                   |
| `oxphp_shared_pool_wait_seconds`            | histogram | `pool_id`                             |

`oxphp_shared_pool_size` 是 `oxphp_shared_pool_count` 的旧拼写；在
deprecation 周期内两条序列携带相同的值，并在未来某个发布移除别名
时分离。新仪表盘请配置 `_count`。

`oxphp_shared_pool_evicted_total` 标签：`reason=idle_timeout | manual | shutdown | dead_owner`。`dead_owner` 标签计数混沌回收时的事件。

### Counter / Flag / Once / Mutex

每实例的 counter、flag、once 和 mutex 不发布单独的指标序列——那会让标签基数膨胀。请使用注册表级 `oxphp_shared_operations_total{type=...}` 计数器，并通过 `/__ox_shared/entry?id=…` JSON 做按实例检视。

> **Mutex 指标是 v1.x 候选项。** 作为后续工作跟踪；今天的可见性通过 `/__ox_shared/entry`。

## 诊断手册

### Pool 已饱和（429 与重试相继失败）

症状：HTTP 调用方看到超时，`oxphp_shared_pool_waiting` 攀升，`oxphp_shared_pool_count` 钉在 `maxSize`。

检查：

```bash
curl -s http://localhost:9090/__ox_shared/entry?id=<pool_id> | jq .type_specific
```

查看 `idle_by_thread`。如果它是 `{}` 或严重不均衡（worker 0 有 8 个 idle、worker 3 有 0 个），说明获取在与正忙于其他事务的线程争用——v1 的按线程亲和性不会再均衡。要么调大 `maxSize`，要么减少按线程的获取热点。

如果 `idle_by_thread` 平衡但所有都在 `in_use`，请调大 `maxSize`。

### 内存饱和

检查 `oxphp_shared_total_bytes` 和 `oxphp_shared_capacity_saturation{kind="bytes"}`。任一偏高时：

1. `curl /__ox_shared/entries?limit=500` 并按 `mem_bytes` 排序找出最大贡献者。
2. 对每个 `curl /__ox_shared/entry?id=<N>` 查看形态。对 Map，查看 `key_count` 与 `max_entries`。
3. 最常见原因：以用户输入为键的无界 `Shared\Map`。修复办法是 `maxEntries` 上限和保留策略。

### 包装无法被垃圾回收

`/__ox_shared/entries` 中的 `refcount` 告诉你还有多少未释放的保留。如果 PHP 包装离开作用域后仍 > 1，说明另一个 Shared 条目还在让它存活。

```bash
curl -s http://localhost:9090/__ox_shared/graph?id=<N> | jq .nodes
```

反向遍历图——任何能到达这个卡住条目的节点都在持有保留。移除该引用（`$map->remove($key)`、关闭通道、丢弃 Mutex 条目），引用计数就会下降。

### 生产中触发了 `CycleException`

异常的消息中包含循环检测器探索过的可达路径。通过 `/__ox_shared/entries` 把那些 ID 映射回类型，然后向 `/__ox_shared/graph?id=<root>` 询问完整形态：

```bash
# 异常消息: "cycle would form: #42 → #51 → #42"
curl -s http://localhost:9090/__ox_shared/graph?id=42 | jq
```

结果会可视化该链，从而看到无意中的反向引用是从哪里引入的。

### 死锁检测器触发

`oxphp_shared_deadlock_detected_total` 在递增。检查服务器日志——检测器会按循环输出一条日志，包含相关 mutex ID 和持有线程。恢复：

1. 对每个 `curl /__ox_shared/entry?id=<mutex_id>` —— 确认 `poisoned=false`。如果已毒化，说明检测器已经中止了循环。
2. 如果是真正的重入 bug，重构为按锁作用域使用独立 mutex。
3. 在预生产中调高 `SHARED_LOCK_DIAGNOSTICS=strict`，把未来的重入从「事后检测的循环」变为「快速失败」。

## 长跑浸泡测试装置

`tests/soak/pool_soak.sh` 是手动（非 CI）装置，用于在数小时或数天的持续负载下验证 Shared\Pool 的稳定性。它：

1. 以动态工作线程伸缩（默认 `PHP_WORKERS=4:40`）和较短的池 `idleTimeout` 启动开发镜像，从而让驱逐调度器不停触发。
2. 加载 `tests/soak/workload.php` 作为 Worker 引导脚本，构建 10 个池 × `maxSize=8`，并对每次请求执行 acquire/release。
3. 用 `wrk` 持续 `SOAK_DURATION_MIN` 分钟（默认 1440 = 24 小时）施加流量。
4. 每 60 秒抓取 `/metrics` 和容器 RSS，写入 `tests/soak/out/<timestamp>/metrics.csv`。
5. 在结束时写出 `verify.txt`，对五条发布退出准则给出通过/失败（RSS 漂移在 ±5% 内、零 stale-handle panic、关停时零泄漏条目、空闲超时驱逐平稳上升、零死锁检测器触发）。

主机上的前提：`docker`、`wrk`、`curl`、`awk`。

典型调用：

```bash
# 发布前的 24 小时完整浸泡
tests/soak/pool_soak.sh

# 用于验证装置自身的 1 小时小压
SOAK_DURATION_MIN=60 tests/soak/pool_soak.sh

# 更高并发
SOAK_CONCURRENCY=400 SOAK_THREADS=8 tests/soak/pool_soak.sh
```

产物落到 `tests/soak/out/<timestamp>/`：

- `metrics.csv` —— 每分钟一行（unix ts、RSS、按类型条目数、按池驱逐计数、死锁数、ops）。
- `server.log` —— 容器 stdout/stderr，含任何 stale-handle 或 panic 痕迹。
- `wrk.out` / `wrk.err` —— 原始压测器输出。
- `metrics.final` —— 容器拆除前最后一次 `/metrics` 抓取。用于读取 `oxphp_shared_leaked_entries_at_shutdown_total`。
- `verify.txt` —— 五项退出准则的通过/失败报告。

请**不要**把它接入 CI。24 小时跑成本不便宜，且其目的是发布前信心，而非持续验证。

## 抓取节奏

注册表端点会在读锁下遍历在线状态，因此抓取廉价但并非免费。推荐节奏：

- `/metrics` —— **每 15 秒**（典型的 Prometheus 默认）。仅聚合；开销可忽略。
- `/__ox_shared/summary` —— **每 60 秒**用于仪表板。比 `/metrics` 略重。
- `/__ox_shared/entries` —— **按需**抓取。会迭代所有分片；不要每个 tick 都抓。
- `/__ox_shared/entry` / `/preview` / `/graph` —— 排查时**按需**。

## 相关

- [共享状态](shared-state.md) —— 心智模型与原语概览。
- [Prometheus 指标](../operations/metrics.md) —— 同一 `/metrics` 端点下的核心服务器指标。
- [内部服务器](../features/internal-server.md) —— `/__ox_shared/*` 端点如何接入 `INTERNAL_ADDR`。
- [迁移到外部存储](migrating-to-external-store.md) —— 当饱和是结构性而非可调时。
