<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('restart_runs_after_the_task', 'opcacherestart');

// Third of three: with the task gone the restart runs.
$after = ox_poll_state(
    static fn (array $s): bool => $s['manual_restarts'] === 2 && $s['opcache_enabled'],
    10.0,
);
$t->assertSame('the restart has run', $after['manual_restarts'] ?? null, 2);
$t->assertSame('a request started after it runs with the cache', $after['opcache_enabled'] ?? null, true);

$t->done();
