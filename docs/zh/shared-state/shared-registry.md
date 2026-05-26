---
title: Shared\Registry
description: Named process-global handles for Shared\* primitives — one Map / Counter / Channel that every worker and every request converges on, without external stores.
---

<!--
TRANSLATION-PENDING

This file is a placeholder. The English source is the authoritative
content at this point; the Chinese translation has not yet been done by
a native translator. The English text is included below verbatim so
intra-cluster links keep resolving and tooling that expects a
shared-registry.md in this language does not 404.

Please replace the body below with a faithful translation following the
conventions of the other zh/shared-state/*.md files.
-->

# Shared\Registry

`OxPHP\Shared\Registry` is the **name-keyed** companion to the rest of `OxPHP\Shared\*`. Where `new Shared\Map()` produces an anonymous entry shared only by handle propagation (`use` capture, async fibers, nesting), `Registry::map('cache', fn() => new Shared\Map(...))` binds an entry under a string key — and every caller of `Registry::map('cache', …)`, on any worker thread, in any request, gets the same entry.

It is the answer to the question *"how do I share one `Shared\Map` across all workers, or across all requests in traditional mode?"*. The other `Shared\*` types are still themselves the right unit of mutable state; `Registry` is just how you put a name on one of them.

See [the English version](../../en/shared-state/shared-registry.md) for the full reference, including the complete exception table (`TypeException`, `CapacityException`, the two `DeadlockException` variants — reentrant and cross-key-cycle — and the two `SharedException` cases for draining and bind-race). Translation pending.

## See also

- [Shared State](shared-state.md)
- [Shared\Map](shared-map.md), [Shared\Counter](shared-counter.md), [Shared\Pool](shared-pool.md)
- [Shared Observability](shared-observability.md)
- [Migrating to an external store](migrating-to-external-store.md)
