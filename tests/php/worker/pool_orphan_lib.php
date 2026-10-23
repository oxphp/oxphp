<?php

declare(strict_types=1);

// Shared by test_pool_orphaned_when_its_creator_exits and its peer endpoint:
// which worker runs the check depends on which one the server hands the
// request to, so both have to be able to run it.

use OxPHP\Shared\Map;
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

if (!class_exists('OxphpPoolOrphanHolder', false)) {
    final class OxphpPoolOrphanHolder
    {
        /** The pool, held by the worker that did not create it. */
        public static ?Pool $pool = null;
    }

    function oxphp_pool_orphan_state(): Map
    {
        return Registry::map('worker-pool-orphan-state', static fn (): Map => new Map());
    }

    /** $destroy of both factories: notes whose slot it was given. */
    function oxphp_pool_orphan_destroy(\stdClass $slot): void
    {
        oxphp_pool_orphan_state()->set('destroyed_' . $slot->madeBy, true);
    }

    function oxphp_pool_orphan_skipped(): int
    {
        $ctx = stream_context_create(['http' => ['timeout' => 3]]);
        $body = (string) @file_get_contents('http://127.0.0.1:9090/metrics', false, $ctx);
        return preg_match('/^oxphp_shared_pool_destroy_skipped_total\{reason="request_ended"\} (\d+)$/m', $body, $m)
            ? (int) $m[1]
            : -1;
    }

    /** On the peer, while the creator runs: make a slot and keep it idle here. */
    function oxphp_pool_orphan_mint(string $name): array
    {
        $pool = Registry::pool($name, static fn () => throw new \LogicException('the test did not create the pool'));
        OxphpPoolOrphanHolder::$pool = $pool;
        $h = $pool->acquire();
        $madeBy = $h->get()->madeBy;
        $h->release();
        return ['minted' => $madeBy, 'idle' => $pool->stats()->idle()];
    }

    /** On the peer, once the creator has exited. */
    function oxphp_pool_orphan_check(string $name): array
    {
        $pool = OxphpPoolOrphanHolder::$pool;
        $out = ['held' => $pool !== null];
        if ($pool === null) {
            return $out;
        }

        $kept = $pool->tryAcquire();
        $out['idle_slot'] = $kept?->get()->madeBy;
        try {
            $made = $pool->tryAcquire();
            $out['new_slot'] = $made === null ? 'none' : 'made by ' . $made->get()->madeBy;
            $made?->release();
        } catch (\Throwable $e) {
            $out['new_slot'] = $e::class . ': ' . $e->getMessage();
        }
        $kept?->release();

        $before = oxphp_pool_orphan_skipped();
        $out['evicted'] = $pool->evict();
        $out['skipped'] = oxphp_pool_orphan_skipped() - $before;
        $out['creator_destroy_ran'] = oxphp_pool_orphan_state()->get('destroyed_creator') ?? false;

        $again = Registry::pool($name, static fn (): Pool => new Pool(
            factory: static fn (): \stdClass => (object) ['madeBy' => 'peer'],
            destroy: 'oxphp_pool_orphan_destroy',
            maxSize: 4,
        ));
        $out['same_pool'] = $again->id() === $pool->id();
        $h = $pool->acquire();
        $out['slot_after_registry'] = $h->get()->madeBy;
        $h->release();
        $out['evicted_after_registry'] = $pool->evict();
        $out['peer_destroy_ran'] = oxphp_pool_orphan_state()->get('destroyed_peer') ?? false;

        Registry::remove($name);
        OxphpPoolOrphanHolder::$pool = null;
        return $out;
    }
}
