<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Map;
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// Outside worker mode a pool's factory and idle slots live in the heap of the
// request that created it, and no other thread can free memory there. Here the
// last reference to such a pool goes away in another request, on the other
// worker, while this request is still running: that request lets the pool go
// without freeing anything, and the server keeps serving — this request
// included.
$t = new TestCase('test_pool_dropped_by_another_request', 'pooltrad');

$key = 'pooltrad-dropped-' . bin2hex(random_bytes(6));
$fixture = static function (string $query): ?array {
    $ch = curl_init('http://127.0.0.1/tests/pooltrad/fixture_pool.php?' . $query);
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 10]);
    $body = curl_exec($ch);
    $decoded = is_string($body) ? json_decode($body, true) : null;
    return is_array($decoded) ? $decoded : null;
};

$map = Registry::map($key, static fn (): Map => new Map());
$pool = new Pool(factory: static fn (): \ArrayObject => new \ArrayObject(['marker' => 'outer']), destroy: null, maxSize: 2);
$h = $pool->acquire();
$h->release();
$t->assertSame('the pool keeps one slot idle', $pool->stats()->idle(), 1);
$map->set('pool', $pool);
unset($h, $pool);

$other = $fixture('step=drop&key=' . $key);
$t->assertNotNull('the other request answered', $other);
$t->assertTrue(
    'the other request ran on the other worker, while this one is running',
    is_int($other['worker'] ?? null) && $other['worker'] !== oxphp_worker_id()
);
$t->assertTrue('it removed the pool from the Map', $other['removed'] ?? false);
$t->assertSame('the Map no longer holds it', $map->count(), 0);

$t->done();
