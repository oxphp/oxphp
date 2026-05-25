---
title: Migrating Shared\* to an external store
description: When to promote in-process Shared\* state to Redis, NATS, or another durable store — the patterns, the semantic gaps, and the concrete migration shape for each Shared\* type.
---

# Migrating Shared\* to an external store

`OxPHP\Shared\*` is in-process. That makes it fast and dependency-free, but it caps you at one host and one process lifetime. This doc is the escape hatch: when you need multi-host coordination or durability across restarts, here is how to move each Shared type to a Redis or NATS (or similar) backend without rewriting your application.

## When to migrate

You probably do not need to migrate. The sweet spot for `Shared\*` — single-host, ephemeral, microsecond-latency coordination — covers more production use cases than people assume. Move to an external store only when one of these is true:

1. **You run more than one OxPHP process.** Multiple hosts, blue/green deploys with overlap, or sidecars that need to see the same state. `Shared\*` is process-local; it cannot cross process boundaries.
2. **State must survive restarts.** A rolling deploy, a crash, or a routine restart loses every `Shared\*` entry. If the loss is unacceptable (billing counters, daily quotas, work-queue positions), you need durability.
3. **State must survive the host.** If any of your hosts can disappear and the state still needs to exist, it lives somewhere other than this host.
4. **You want cross-language readers.** An external store can be read by a background job written in Go, a metrics pipeline, or an admin tool. `Shared\*` is PHP-only.

If none of those apply, the in-process primitive is almost certainly the right choice. Keep the migration plan in your back pocket instead of your hot path.

## The abstraction

Most teams adopt the same shape: an interface with two backends, chosen by configuration.

```php
<?php
interface CounterBackend
{
    public function inc(string $key, int $by = 1): int;
    public function get(string $key): int;
    public function reset(string $key): int;
}

final class SharedCounterBackend implements CounterBackend
{
    public function __construct(private OxPHP\Shared\Map $counters) {}

    public function inc(string $key, int $by = 1): int
    {
        $counter = $this->counters->getOrSet(
            $key,
            fn () => new OxPHP\Shared\Counter(),
        );
        return $counter->add($by);
    }

    public function get(string $key): int
    {
        $counter = $this->counters->get($key);
        return $counter?->get() ?? 0;
    }

    public function reset(string $key): int
    {
        $counter = $this->counters->get($key);
        return $counter?->set(0) ?? 0;
    }
}

final class RedisCounterBackend implements CounterBackend
{
    public function __construct(private Redis $redis) {}

    public function inc(string $key, int $by = 1): int
    {
        return (int) $this->redis->incrBy("counter:{$key}", $by);
    }

    public function get(string $key): int
    {
        return (int) ($this->redis->get("counter:{$key}") ?? 0);
    }

    public function reset(string $key): int
    {
        // GETSET is atomic: one round-trip, returns the prior value.
        return (int) ($this->redis->getSet("counter:{$key}", 0) ?? 0);
    }
}
```

Wire the chosen backend once at bootstrap and use `CounterBackend` everywhere. Migration is then a configuration flip, not a rewrite.

## Per-type migration notes

Each `Shared\*` type has semantic quirks that do not translate trivially to any external store. The notes below call out the differences and the idiomatic replacements.

### `Shared\Counter` → Redis / NATS JetStream KV

- **Redis:** `INCR` / `INCRBY` / `GET`. Atomic, durable, and replicated in Redis Cluster.
- **NATS JetStream KV:** `KV.put` with revision-based CAS covers both `set` and `compareAndSet`. Increments require `KV.get` + `KV.update(revision)` in a loop.

Semantic gaps:

- Batch accumulation is `add(array_sum($deltas))` — one FFI round trip in `Shared\*`. In Redis, precompute the sum and do one `INCRBY` (one RTT); in NATS it is one `KV.update`.
- Integer overflow in Redis returns an error; `Shared\Counter` wraps silently.

### `Shared\Flag` → Redis / NATS feature-flag service

- **Redis:** `SET` / `GET` / `SETNX` for `compareAndSet`-ish semantics. A string value `"1"` / `"0"` works; booleans are cleaner via `GETSET` + string comparison.
- **Dedicated flag service:** (LaunchDarkly, Unleash, ConfigCat) handles the cache, rollout targeting, and audit trail out of the box. For operational kill-switches this is usually the right move once you cross the `Shared\*` threshold.

Semantic gaps:

- `swap($new)` → Redis `GETSET`. Atomic.
- `compareAndSet($expect, $new)` → Lua script or `WATCH`/`MULTI`. Worth wrapping as a helper.
- External flag services usually cache the value locally; your read is not always a network round trip. That is typically fine, but expect eventual consistency on changes.

### `Shared\Once` → Database bootstrap table

- **Pattern:** idempotent INSERT with a unique constraint, then SELECT on conflict.
- **SQL:** `INSERT INTO once (key, value) VALUES (?, ?) ON CONFLICT (key) DO NOTHING; SELECT value FROM once WHERE key = ?`.
- **Redis:** `SETNX` + `GET`.

Semantic gaps:

- `Shared\Once::getOrInit(callable)` runs the factory in-process when it wins. In an external store the factory must be idempotent (two writers may both run it and only one value wins) or you need a leader-election wrapper.
- `DeadlockException` on reentrance has no external equivalent — you inherit whatever the store does, which is typically nothing.

### `Shared\Mutex` → Redis distributed lock

- **Redis:** the "Redlock" pattern, or the simpler `SET NX EX` single-key lock if your guarantees are relaxed. Libraries like `cheprasov/php-redis-lock` wrap this.
- **etcd / Consul / Zookeeper:** session-based locks with lease renewal. More operational overhead but stronger guarantees.

Semantic gaps:

- **This is the hardest migration.** In-process mutexes are instantaneous and correct; distributed locks are slow and offer only best-effort guarantees. Assume the semantics will change — design for at-least-once, idempotent critical sections.
- `with($fn)` in `Shared\Mutex` atomically commits the closure's return value back to the guarded storage. With a Redis lock you must explicitly read, compute, then write, and the write can race with an unrelated operation.
- Poisoning: external locks do not have a "poisoned" state. If your closure throws in a distributed critical section, you release the lock and let the next caller see half-committed state. Handle consistency via a compensating action, not by mimicking `isPoisoned()`.

### `Shared\Channel` → NATS JetStream / Redis Streams / SQS / Kafka

- **NATS JetStream:** the closest semantic match. Durable, bounded, MPMC, with consumer offsets and at-least-once delivery.
- **Redis Streams:** `XADD` / `XREADGROUP` covers the basic queue pattern. Consumer groups match `Shared\Channel`'s multi-consumer semantics.
- **SQS / Kafka:** industry staples. Kafka is the right choice for high-throughput event streams; SQS for simple work queues.

Semantic gaps:

- **Blocking `recv` is replaced by long polling.** Your consumer code changes from "return null on close" to "poll with a timeout, handle reconnect."
- **`sendMany` batching** maps to Kafka's linger/batch config or Redis pipelining.
- **`close()`** has no external analog. Stop producers gracefully and let consumers drain; there is no signal that says "no more items ever."
- **In-process ordering** becomes at-least-once delivery across a network. Idempotency keys on the consumer side are mandatory.

### `Shared\Map` → Redis hash / a KV service / a database

- **Redis hash:** `HGET` / `HSET` / `HDEL` / `HSCAN` covers the keyed-map shape.
- **Keyed string values:** `SET key:<k> value` with a `maxEntries` enforced via LRU eviction.
- **Database table with TTL column:** rows are entries; a background sweeper handles eviction. This is what you want when the values are larger than a few hundred bytes.

Semantic gaps:

- `update($key, $fn)` must become a server-side Lua script in Redis (to keep the RMW atomic) or a `SELECT ... FOR UPDATE` in SQL. Plain `HGET` + compute + `HSET` loses atomicity.
- Map's **cycle safety** does not exist externally. You will never close a cycle because there is no Shareable graph to close.
- **Nested Shareables** become "separate key with a pointer encoded in the value". You own the bookkeeping.

### `Shared\Pool` → Client library pools

- **Prefer the library's own pool.** PDO, Guzzle, HTTP clients, and most database drivers have mature pooling. Do not reinvent them with a `Shared\Pool`.
- **Proxy services:** for per-host Postgres/MySQL pooling, pgbouncer / proxysql terminate the pooling boundary at the infrastructure layer. Your PHP side becomes stateless again.

Semantic gaps:

- Pool idle-timeout eviction is replaced by the library's own health checking.
- Factory/destroy callbacks are replaced by the library's connection lifecycle.
- Cross-host, you may need **per-service** pools (one per downstream) rather than one big pool.

## A concrete case: per-tenant rate limiter

Here is the [rate-limiter example](shared-state.md#canonical-example--migrating-a-hand-rolled-counter) from `shared-state.md` reworked behind a backend interface:

```php
<?php
interface RateLimiterBackend
{
    public function allow(string $key, int $max, int $windowSecs): bool;
}

final class SharedRateLimiterBackend implements RateLimiterBackend
{
    public function __construct(private OxPHP\Shared\Map $buckets) {}

    public function allow(string $key, int $max, int $windowSecs): bool
    {
        $now = time();
        $state = $this->buckets->update($key, function ($current) use ($now, $windowSecs) {
            if ($current === null || $now - $current['start'] >= $windowSecs) {
                return ['count' => 1, 'start' => $now];
            }
            return ['count' => $current['count'] + 1, 'start' => $current['start']];
        });
        return $state['count'] <= $max;
    }
}

final class RedisRateLimiterBackend implements RateLimiterBackend
{
    /**
     * Atomic fixed-window counter. Load this script once at bootstrap
     * via `$redis->script('load', $lua)` and keep the resulting SHA.
     */
    private const SCRIPT = <<<'LUA'
        local current = redis.call('GET', KEYS[1])
        if current then
            local c = tonumber(current) + 1
            redis.call('SET', KEYS[1], c, 'KEEPTTL')
            return c
        end
        redis.call('SET', KEYS[1], 1, 'EX', ARGV[1])
        return 1
    LUA;

    public function __construct(
        private Redis $redis,
        private string $scriptSha,
    ) {}

    public static function withLoadedScript(Redis $redis): self
    {
        $sha = $redis->script('load', self::SCRIPT);
        return new self($redis, $sha);
    }

    public function allow(string $key, int $max, int $windowSecs): bool
    {
        $count = (int) $this->redis->evalSha($this->scriptSha, ["rl:{$key}"], [$windowSecs]);
        return $count <= $max;
    }
}
```

The only thing that changes between single-host and multi-host deploys is which backend is wired up at bootstrap. The rest of the app talks to `RateLimiterBackend`.

## Hybrid patterns

### Local cache in front of external state

Read-heavy workloads often use `Shared\Map` as a TTL cache in front of an external store. You hit Redis once every N seconds; you hit `Shared\Map` thousands of times a second.

```php
<?php
$cfg = $cache->getOrSet($tenantId, fn () => loadFromRedis($tenantId));
```

Invalidate via a Redis pub/sub channel that all OxPHP processes subscribe to, or via a TTL in the local Map.

### Write-through buffer

Write-heavy workloads buffer in a `Shared\Channel` and a background consumer flushes to the external store. You absorb bursts in-process and amortise network overhead.

```php
<?php
$writes = new OxPHP\Shared\Channel(capacity: 10_000);

oxphp_async(function () use ($writes) {
    while (($batch = $writes->recvMany(max: 100, timeout: 0.5))) {
        writeBatchToRedis($batch);
    }
});

// Hot path
$writes->trySend([$key, $value]);
```

Trade-off: if the process dies before the flush completes you lose the buffered items. Appropriate for analytics, not for billing.

## Checklist

Before you cut over:

- [ ] Identify the one `Shared\*` primitive behind the migration. Do not migrate "everything" at once.
- [ ] Extract an interface; wire both backends.
- [ ] Decide consistency — at-most-once or at-least-once — and make it explicit in the interface.
- [ ] Test both backends with the same integration test suite.
- [ ] Measure latency. External stores add 0.1–5 ms per op — verify your app can absorb it on hot paths.
- [ ] Plan for the external store being down: fail open (let the request through) or fail closed (serve 503)? The right answer is domain-specific.
- [ ] Enable `oxphp_shared_*` metrics on the `Shared\*` backend before and after the cut-over so you can compare.

## Related

- [Shared State](shared-state.md) — overview; when to stay in-process.
- [Shared Observability](shared-observability.md) — instrument both backends the same way.
- [Rate Limiting](../features/rate-limiting.md) — the built-in per-IP limiter (runs before PHP; orthogonal to PHP-level limits).
