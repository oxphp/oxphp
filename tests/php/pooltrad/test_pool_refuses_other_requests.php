<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\ClosedException;
use OxPHP\Shared\Map;
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

// Outside worker mode a pool's factory and slots live in the heap of the
// request that created it. Another request — here one running at the same
// time, on the other worker — reaches the pool through a Shared\Map and is
// refused with a ClosedException that says why, instead of running this
// request's factory on its own thread. This request goes on using the pool.
$t = new TestCase('test_pool_refuses_other_requests', 'pooltrad');

$key = 'pooltrad-other-' . bin2hex(random_bytes(6));
$fixture = static function (string $query): ?array {
    $ch = curl_init('http://127.0.0.1/tests/pooltrad/fixture_pool.php?' . $query);
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 10]);
    $body = curl_exec($ch);
    $decoded = is_string($body) ? json_decode($body, true) : null;
    return is_array($decoded) ? $decoded : null;
};

$pool = new Pool(factory: static fn (): \ArrayObject => new \ArrayObject(['marker' => 'outer']), destroy: null, maxSize: 2);
Registry::map($key, static fn (): Map => new Map())->set('pool', $pool);
$h = $pool->acquire();
$mine = $h->get();
$h->release();

$other = $fixture('step=use&key=' . $key);
$t->assertNotNull('the other request answered', $other);
$t->assertTrue(
    'the other request ran on the other worker, while this one is running',
    is_int($other['worker'] ?? null) && $other['worker'] !== oxphp_worker_id()
);
$t->assertFalse('the other request got no slot', array_key_exists('acquired', $other ?? []));
$t->assertSame('the other request is refused with a ClosedException', $other['threw'] ?? null, ClosedException::class);
$t->assertContains('the refusal says why', $other['message'] ?? '', 'outside worker mode');

$h = $pool->acquire();
$t->assertSame('this request still gets its own slot back', $h->get(), $mine);
$h->release();

$t->done();
