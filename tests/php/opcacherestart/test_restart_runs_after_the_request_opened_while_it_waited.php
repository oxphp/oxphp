<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('restart_runs_after_the_request_opened_while_it_waited', 'opcacherestart');

// Third of three: with that request gone the restart runs, and the pool
// threads, replaced once more, run with the cache.
$after = ox_poll_state(
    static fn (array $s): bool => $s['manual_restarts'] === 4 && $s['opcache_enabled'],
    10.0,
);
$t->assertSame('the restart has run', $after['manual_restarts'] ?? null, 4);
$t->assertSame('a request started after it runs with the cache', $after['opcache_enabled'] ?? null, true);
$t->assertTrue('both pool threads run tasks with the cache again', ox_poll_tasks(true, 10.0));

$t->done();
