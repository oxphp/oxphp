<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\ClosedException;

// Outside worker mode the heap a pool's slots and factory live in is gone once
// the request that created the pool ends. A later request on the same thread,
// reaching the pool through a Shared\Map, finds no slot left, nothing to
// evict, and a ClosedException that says why on acquire — never the slot the
// first request left idle, nor the one it still held when it ended.
//
// This test holds one of the profile's two workers, so both requests it sends
// run one after the other on the other worker.
$t = new TestCase('test_pool_closes_with_its_request', 'pooltrad');

$key = 'pooltrad-ended-' . bin2hex(random_bytes(6));
$fixture = static function (string $query): ?array {
    $ch = curl_init('http://127.0.0.1/tests/pooltrad/fixture_pool.php?' . $query);
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 10]);
    $body = curl_exec($ch);
    $decoded = is_string($body) ? json_decode($body, true) : null;
    return is_array($decoded) ? $decoded : null;
};

$created = $fixture('step=create&key=' . $key);
$t->assertNotNull('the creating request answered', $created);
$t->assertSame('the creating request left one slot idle', $created['idle'] ?? null, 1);
$t->assertSame('and held another until it ended', $created['size'] ?? null, 2);

$later = $fixture('step=use&stale=check&key=' . $key);
$t->assertNotNull('the later request answered', $later);
$t->assertTrue(
    'both requests ran on the same worker, not this one',
    is_int($created['worker'] ?? null)
        && ($later['worker'] ?? null) === $created['worker']
        && $created['worker'] !== oxphp_worker_id()
);
$t->assertSame('no slot outlived the creating request', $later['idle'] ?? null, 0);
$t->assertSame('none is counted as live', $later['size'] ?? null, 0);
$t->assertSame('there is nothing to evict', $later['evicted'] ?? null, 0);
$t->assertFalse('the later request got no slot', array_key_exists('acquired', $later ?? []));
$t->assertSame('the later request is refused with a ClosedException', $later['threw'] ?? null, ClosedException::class);
$t->assertContains('the refusal says why', $later['message'] ?? '', 'outside worker mode');

$t->done();
