<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('restart_runs_after_the_long_tasks', 'opcacherestart');

// With the long tasks and the request that dispatched them gone, nothing holds
// the restart any more, and the threads reach the moment they retire at.
$before = ox_get_json('/tests/opcacherestart/fixture_status.php');
$after = ox_poll_state(
    static fn (array $s): bool => !$s['restart_pending'] && $s['opcache_enabled'],
    10.0,
);
$t->assertSame('no restart is pending', $after['restart_pending'] ?? null, false);
$t->assertSame('a request started after it runs with the cache', $after['opcache_enabled'] ?? null, true);
$t->assertTrue('both pool threads run tasks with the cache again', ox_poll_tasks(true, 10.0));
$t->meta('state_before', $before);

$t->done();
