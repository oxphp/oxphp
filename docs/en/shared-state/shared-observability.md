---
title: Shared\* Observability
description: Introspection endpoints, Prometheus metrics, and diagnostic playbooks for the OxPHP\Shared\* primitives — inspect live registry entries, trace reachability graphs, and alert on saturation.
---

# Shared\* Observability

Every `OxPHP\Shared\*` instance is a registry entry the runtime already tracks for refcount and capacity. That tracking is exposed to operators as JSON introspection under `/__ox_shared/*` and as Prometheus metrics under `oxphp_shared_*`. This doc is the reference and the field guide.

## Enabling

Observability piggybacks on the [internal server](../features/internal-server.md). Set `INTERNAL_ADDR` to start it:

```bash
INTERNAL_ADDR=127.0.0.1:9090
```

Both the JSON endpoints and `/metrics` are then reachable at that address. No additional configuration is required.

You can disable either independently:

| Env var                         | Default | Effect                                                       |
|---------------------------------|---------|--------------------------------------------------------------|
| `SHARED_INTROSPECTION_ENABLED`  | `true`  | Toggles the `/__ox_shared/*` JSON API.                       |
| `SHARED_INTROSPECTION_PREVIEW_ENABLED` | `true` | Toggles `/preview` (value-shape previews can leak data). |
| `SHARED_METRICS_ENABLED`        | `true`  | Toggles the `oxphp_shared_*` Prometheus metrics.             |

Turn introspection off in hostile-tenant deployments; metrics are aggregate-only and safe to keep on.

## Introspection endpoints

All responses are `Content-Type: application/json; charset=utf-8`. Query parameters are standard URL-encoded.

### `GET /__ox_shared/summary`

Top-level snapshot: aggregate counts per type, memory, op rate, and saturation against the configured caps.

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

Use `summary` in dashboards and cron alerts. A single scrape gives you per-type health and capacity headroom.

### `GET /__ox_shared/entries?limit=N`

Lists live entries (capped at `limit`, default 100, max 500). One line per entry:

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

`refcount` is the external retain count — how many PHP wrappers and nested Shared entries hold this one. When you expect an entry to be GC-able but it is not, this is the field to check.

### `GET /__ox_shared/entry?id=N`

Type-specific detail for one entry:

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

`type_specific` varies by type — Pool exposes `{ size, in_use, idle, waiting, idle_by_thread, max_size }`, Channel exposes `{ capacity, pending, closed, senders_blocked, receivers_blocked }`, Counter exposes `{ value }`, and so on.

### `GET /__ox_shared/preview?id=N`

Value-shape preview of scalar and small-array values. String values are truncated to `SHARED_PREVIEW_STRING_LIMIT` (default 256 bytes); arrays show the first `SHARED_PREVIEW_ARRAY_LIMIT` entries (default 20). Gated by `SHARED_INTROSPECTION_PREVIEW_ENABLED`.

```json
{ "id": 42, "type": "Counter", "preview": "1420" }
```

Use `preview` during development; disable it in production when values may contain user data.

### `GET /__ox_shared/types`

Enumerates the v1 type catalog — useful for generated tooling that needs the tag → class mapping:

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

BFS walk of outgoing `Shareable` references starting at `id=N`. Returns nodes and edges of the reachable subgraph. Defaults: `depth=16`, `edges=500`. Walker-budget hits set `truncated: true` in the response.

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

Reach for `graph` after a `CycleException` to see the reachable path the walker took, or when diagnosing "why is this Counter not getting GC'd" — the graph shows every parent holding a retain on it.

## Prometheus metrics

All metrics are exposed at `GET /metrics` alongside the core server metrics.

### Registry-wide

| Metric                                         | Type    | Labels          | Description                                        |
|------------------------------------------------|---------|-----------------|----------------------------------------------------|
| `oxphp_shared_objects_total`                   | gauge   | `type`          | Live entry count per type.                         |
| `oxphp_shared_operations_total`                | counter | `type`          | Cumulative ops dispatched to each type.            |
| `oxphp_shared_bytes`                           | gauge   | `type`          | Approximate bytes per type (±30% vs `mallinfo`).   |
| `oxphp_shared_total_bytes`                     | gauge   | —               | Sum across types.                                  |
| `oxphp_shared_capacity_saturation`             | gauge   | `kind`          | `entries` and `bytes` as fractions of their caps.  |
| `oxphp_shared_deadlock_detected_total`         | counter | —               | Cross-thread wait-for cycles detected.             |

### Channel

| Metric                                           | Type    | Labels        |
|--------------------------------------------------|---------|---------------|
| `oxphp_shared_channel_count`                     | gauge   | `channel_id`  |
| `oxphp_shared_channel_pending` *(deprecated)*    | gauge   | `channel_id`  |
| `oxphp_shared_channel_senders_blocked`           | gauge   | `channel_id`  |
| `oxphp_shared_channel_receivers_blocked`         | gauge   | `channel_id`  |
| `oxphp_shared_channel_items_sent_total`          | counter | `channel_id`  |
| `oxphp_shared_channel_items_dropped_total`       | counter | `channel_id`  |

`oxphp_shared_channel_pending` is the legacy spelling of
`oxphp_shared_channel_count`; both series carry the same value during
the deprecation window and will diverge when the alias is removed in
a future release. Wire new dashboards against `_count`.

### Map

| Metric                                  | Type    | Labels     |
|-----------------------------------------|---------|------------|
| `oxphp_shared_map_entries`              | gauge   | `map_id`   |
| `oxphp_shared_map_max_entries`          | gauge   | `map_id`   |
| `oxphp_shared_map_saturation`           | gauge   | `map_id`   |

### Pool

| Metric                                    | Type      | Labels                              |
|-------------------------------------------|-----------|-------------------------------------|
| `oxphp_shared_pool_count`                 | gauge     | `pool_id`                           |
| `oxphp_shared_pool_size` *(deprecated)*   | gauge     | `pool_id`                           |
| `oxphp_shared_pool_in_use`                | gauge     | `pool_id`                           |
| `oxphp_shared_pool_idle`                  | gauge     | `pool_id`                           |
| `oxphp_shared_pool_waiting`               | gauge     | `pool_id`                           |
| `oxphp_shared_pool_acquire_total`         | counter   | `pool_id`                           |
| `oxphp_shared_pool_evicted_total`         | counter   | `pool_id`, `reason`                 |
| `oxphp_shared_pool_wait_seconds`          | histogram | `pool_id`                           |

`oxphp_shared_pool_size` is the legacy spelling of
`oxphp_shared_pool_count`; both series carry the same value during
the deprecation window and will diverge when the alias is removed in
a future release. Wire new dashboards against `_count`.

`oxphp_shared_pool_evicted_total` labels: `reason=idle_timeout | evict | shutdown`. `idle_timeout` is an automatic eviction of an idle slot, `evict` is an explicit `Pool::evict()` call, and `shutdown` is teardown at process exit.

### Counter / Flag / Once / Mutex

Per-instance counters, flags, onces, and mutexes do not ship individual metric series — they would bloat label cardinality. Use the registry-wide `oxphp_shared_operations_total{type=...}` counter and the `/__ox_shared/entry?id=…` JSON for per-instance inspection instead.

> **Mutex metrics are a v1.x candidate.** Tracked as follow-up work; today's visibility is via `/__ox_shared/entry`.

## Diagnostic playbooks

### Pool is saturated (429s with retries failing)

Symptoms: HTTP callers see timeouts, `oxphp_shared_pool_waiting` climbs, `oxphp_shared_pool_count` is pinned at `maxSize`.

Check:

```bash
curl -s http://localhost:9090/__ox_shared/entry?id=<pool_id> | jq .type_specific
```

Look at `idle_by_thread`. If it is `{}` or heavily imbalanced (worker 0 has 8 idle, worker 3 has 0), the acquire is contending for threads that happen to be busy elsewhere — per-thread affinity in v1 does not rebalance. Either raise `maxSize` or reduce the per-thread acquire hotspot.

If `idle_by_thread` is balanced but everything is in `in_use`, raise `maxSize`.

### Memory saturation

Check `oxphp_shared_total_bytes` and `oxphp_shared_capacity_saturation{kind="bytes"}`. If either is high:

1. `curl /__ox_shared/entries?limit=500` and sort by `mem_bytes` to find the top contributors.
2. `curl /__ox_shared/entry?id=<N>` on each to check the shape. For Map, look at `key_count` vs `max_entries`.
3. Most common cause: an unbounded `Shared\Map` keyed by user input. Remedy is a `maxEntries` cap and a retention policy.

### A wrapper won't get garbage-collected

`refcount` in `/__ox_shared/entries` tells you how many outstanding retains there are. If it stays above 1 after the PHP wrapper leaves scope, another Shared entry is keeping it alive.

```bash
curl -s http://localhost:9090/__ox_shared/graph?id=<N> | jq .nodes
```

Walk the graph backwards — any node reaching the stuck entry is holding a retain. Remove the reference (`$map->remove($key)`, close the channel, drop the Mutex entry) and the refcount drops.

### A `CycleException` fired in production

The exception's message includes the reachable path the cycle detector explored. Map those IDs back to types via `/__ox_shared/entries`, and ask `/__ox_shared/graph?id=<root>` for the full shape:

```bash
# Exception message: "cycle would form: #42 → #51 → #42"
curl -s http://localhost:9090/__ox_shared/graph?id=42 | jq
```

The result visualises the chain so you can see where the inadvertent back-reference was introduced.

### Deadlock detector fired

`oxphp_shared_deadlock_detected_total` is ticking. Check server logs — the detector emits a log record per cycle with the involved mutex IDs and the owning threads. Recover:

1. `curl /__ox_shared/entry?id=<mutex_id>` on each — confirm `poisoned=false`. If poisoned, the detector already aborted the cycle.
2. If the cycle is a real reentry bug, refactor to use separate mutexes per lock scope.
3. Raise `SHARED_LOCK_DIAGNOSTICS=strict` in staging to turn a future reentry into a fast-fail instead of a detected cycle.

## Long-running soak harness

`tests/soak/pool_soak.sh` is a manual (non-CI) harness for verifying the Shared\Pool stability story over hours or days of continuous load. It:

1. Boots the dev image with dynamic worker scaling (`PHP_WORKERS=4:40` by default) and a short pool `idleTimeout` so the eviction scheduler fires continuously.
2. Loads `tests/soak/workload.php` as the worker bootstrap, which constructs 10 pools × `maxSize=8` and serves acquire/release on every request.
3. Drives traffic with `wrk` for `SOAK_DURATION_MIN` minutes (default 1440 = 24h).
4. Scrapes `/metrics` and the container's RSS every 60 s into `tests/soak/out/<timestamp>/metrics.csv`.
5. Writes `verify.txt` at the end with pass/fail for the five release-exit criteria (RSS drift within ±5%, zero stale-handle panics, zero leaked entries at shutdown, idle-timeout evictions rising smoothly, zero deadlock-detector firings).

Prerequisites on the host: `docker`, `wrk`, `curl`, `awk`.

Typical invocations:

```bash
# 24h full soak before a release
tests/soak/pool_soak.sh

# 1h smoke for validating the harness itself
SOAK_DURATION_MIN=60 tests/soak/pool_soak.sh

# Heavier concurrency
SOAK_CONCURRENCY=400 SOAK_THREADS=8 tests/soak/pool_soak.sh
```

Artifacts land in `tests/soak/out/<timestamp>/`:

- `metrics.csv` — one row per minute (unix ts, RSS, per-type entry counts, per-pool eviction counters, deadlock count, ops).
- `server.log` — container stdout/stderr including any stale-handle or panic trails.
- `wrk.out` / `wrk.err` — raw load-generator output.
- `metrics.final` — the last `/metrics` scrape taken just before container teardown. Used to confirm entry and byte counts have returned to baseline (no leaked Shared entries) after the load stops.
- `verify.txt` — pass/fail report for the five exit criteria.

Do **not** wire this into CI. A 24h run is not cheap and its purpose is pre-release confidence, not continuous validation.

## Scrape cadence

The registry endpoints walk live state under read locks, so scraping is cheap but not free. Recommended cadences:

- `/metrics` — **every 15 s** (typical Prometheus default). Aggregate-only; overhead is negligible.
- `/__ox_shared/summary` — **every 60 s** for dashboards. Slightly heavier than `/metrics`.
- `/__ox_shared/entries` — **on demand** only. Iterates all shards; do not scrape every tick.
- `/__ox_shared/entry` / `/preview` / `/graph` — **on demand** during investigations.

## Related

- [Shared State](shared-state.md) — mental model and primitives overview.
- [Prometheus Metrics](../operations/metrics.md) — core server metrics under the same `/metrics` endpoint.
- [Internal Server](../features/internal-server.md) — how the `/__ox_shared/*` endpoints plug into `INTERNAL_ADDR`.
- [Migrating to an external store](migrating-to-external-store.md) — when saturation is structural, not tunable.
