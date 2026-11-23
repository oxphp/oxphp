<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

use OxPHP\Shared\Map;
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

$t = new TestCase('refused_pool_call_does_not_hold_the_restart', 'opcacherestart');

// Second of two. This request opened without the cache while the holding task
// keeps the restart back, so its code is in its own memory, and so is that of
// the task it hands to a pool thread. That task creates two pools on its
// thread: one whose factory is a built-in method of an object only that thread
// can use, which a pool refuses to call on any other thread, and one whose
// factory is a built-in function, which it calls anywhere. This request then
// calls the first, and a request of its own, opened without the cache too,
// calls the second. Neither factory is a closure: one created from this
// request's code would keep pointing into its memory after it has ended.
//
// The call the pool made holds the restart until the request that made it has
// ended; the call it refused ran nothing, and the restart runs while this
// request still runs.
$base = sys_get_temp_dir() . '/oxphp_opcacherestart_poolcall';
$t->assertTrue('premise: the holding task is running', is_file("$base.started"));

$mine = ox_opcache_state();
$t->assertSame('premise: the restart is pending', $mine['restart_pending'], true);
$t->assertSame('premise: this request runs without the cache', $mine['opcache_enabled'], false);
$restarts = $mine['manual_restarts'];

$key = 'opcacherestart-poolcall-' . bin2hex(random_bytes(6));
$map = Registry::map($key, static fn (): Map => new Map());
// The holding task keeps one pool thread busy, so this one runs on the other.
$made = oxphp_async_await(oxphp_async(static function () use ($map): bool {
    $map->set('refused', new Pool(factory: [new \ArrayObject(), 'getIterator'], destroy: null, maxSize: 1));
    $map->set('builtin', new Pool(factory: 'date_create', destroy: null, maxSize: 1));
    return true;
}), 4.0);
$t->assertSame('premise: a pool thread created both pools', $made, true);

try {
    $h = $map->get('refused')->acquire();
    $h->release();
    $refusal = null;
} catch (\Error $e) {
    $refusal = $e->getMessage();
}
$t->assertType('premise: the pool refused this request the call of its factory', $refusal, 'string');
$t->meta('refusal', $refusal);

// The second request, sent and answered later: it holds its connection open
// until `go2`.
$conn = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 5.0);
$t->assertTrue('premise: the second request is connected', is_resource($conn));
fwrite($conn, "GET /tests/opcacherestart/fixture_pool_call_hold.php?key=$key HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
$deadline = microtime(true) + 5.0;
while (!file_exists("$base.held") && microtime(true) < $deadline) {
    usleep(20000);
    clearstatcache();
}
$held = is_file("$base.held") ? json_decode((string) file_get_contents("$base.held"), true) : null;
$t->assertSame('premise: the pool called its factory for the second request', $held['got'] ?? null, \DateTime::class);

touch("$base.go");
$deadline = microtime(true) + 5.0;
while (!file_exists("$base.report") && microtime(true) < $deadline) {
    usleep(20000);
    clearstatcache();
}
$t->assertTrue('premise: the holding task has ended', is_file("$base.report"));

// Long enough for the holding task's thread to be replaced and for requests to
// start after it.
usleep(1500000);
$during = ox_get_json('/tests/opcacherestart/fixture_status.php');
$t->assertSame(
    'the restart waits for the request whose call the pool made',
    $during['manual_restarts'] ?? null,
    $restarts
);

touch("$base.go2");
stream_set_timeout($conn, 10);
$reply = stream_get_contents($conn);
fclose($conn);
$t->assertContains('premise: the second request has ended', (string) $reply, '"got":"DateTime"');

$ran = ox_poll_state(static fn (array $s): bool => $s['manual_restarts'] > $restarts, 10.0);
$t->assertSame(
    'the restart ran while this request, whose call the pool refused, still runs',
    $ran['manual_restarts'] ?? null,
    $restarts + 1
);

$t->done();
