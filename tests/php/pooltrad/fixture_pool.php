<?php

declare(strict_types=1);

use OxPHP\Shared\Map;
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// Driven over HTTP by the pooltrad tests; answers one JSON object.
//
//   step=create  make a pool, hand it out through the Map bound to `key`, and
//                leave one slot idle in it and another held until the request
//                ends
//   step=use     take the pool from that Map and try to acquire from it; with
//                stale=check, stop short of acquiring when the pool still holds
//                an idle slot
//   step=drop    remove the pool from that Map, letting it go
header('Content-Type: application/json');

$key = (string) ($_GET['key'] ?? '');
$map = Registry::map($key, static fn (): Map => new Map());
$out = ['worker' => oxphp_worker_id()];

switch ($_GET['step'] ?? '') {
    case 'create':
        $pool = new Pool(
            factory: static fn (): \ArrayObject => new \ArrayObject(['marker' => 'fixture']),
            destroy: null,
            maxSize: 2,
        );
        $map->set('pool', $pool);
        $h = $pool->acquire();
        $held = $pool->acquire();
        $h->release();
        $out['idle'] = $pool->stats()->idle();
        $out['size'] = $pool->stats()->size();
        break;

    case 'use':
        $pool = $map->get('pool');
        $stats = $pool->stats();
        $out['idle'] = $stats->idle();
        $out['size'] = $stats->size();
        if (($_GET['stale'] ?? '') === 'check' && $out['idle'] !== 0) {
            // A slot an earlier request left behind. Taking it would hand this
            // request whatever sits in that memory now.
            break;
        }
        $out['evicted'] = $pool->evict();
        try {
            $h = $pool->tryAcquire();
            $out['acquired'] = $h === null ? null : get_debug_type($h->get());
        } catch (\Throwable $e) {
            $out['threw'] = get_class($e);
            $out['message'] = $e->getMessage();
        }
        break;

    case 'drop':
        // The Map holds the pool's last reference, so removing it lets the
        // pool go here, on this thread.
        $out['removed'] = $map->remove('pool');
        break;
}

echo json_encode($out);
