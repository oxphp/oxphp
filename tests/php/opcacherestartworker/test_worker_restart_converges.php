<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/../opcacherestart/lib.php';

$t = new TestCase('worker_restart_converges', 'opcacherestartworker');

// Once the worker that held the restart has been replaced, the restart runs,
// and the worker that booted while it waited is replaced again: after that the
// other worker answers with the cache, request after request.
$deadline = microtime(true) + 12.0;
$streak = 0;
$last = null;
do {
    $last = ox_get_json('/tests/opcacherestart/fixture_status.php', 5.0);
    $ok = $last !== null && $last['manual_restarts'] === 1 && $last['opcache_enabled'] && !$last['restart_pending'];
    $streak = $ok ? $streak + 1 : 0;
    if ($streak >= 5) {
        break;
    }
    usleep(200000);
} while (microtime(true) < $deadline);
$t->assertSame('the restart has run and the other worker runs with the cache', $streak >= 5, true);
$t->meta('last', $last);

$t->done();
