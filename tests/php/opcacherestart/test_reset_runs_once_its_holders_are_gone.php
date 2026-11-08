<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('reset_runs_once_its_holders_are_gone', 'opcacherestart');

// The other half of reset_waits_for_the_running_request: once the request that
// held the restart has ended, the restart runs, and the requests and pool
// threads after it run with the cache again.
//
// The pool threads were replaced while that request held the restart, and their
// replacements opened without the cache; once the restart has run they are
// replaced once more. A server that leaves its pool threads alone never runs the
// restart, and the first wait below runs out.
$after = ox_poll_state(
    static fn (array $s): bool => $s['manual_restarts'] === 1 && $s['opcache_enabled'],
    10.0,
);
$t->assertSame('the restart has run, once', $after['manual_restarts'] ?? null, 1);
$t->assertSame('no restart is pending', $after['restart_pending'] ?? null, false);
$t->assertSame('a request started after it runs with the cache', $after['opcache_enabled'] ?? null, true);

// A script first compiled after the restart goes into the cache.
ox_get_json('/tests/opcacherestart/fixture_cache_probe.php');
$cached = ox_get_json('/tests/opcacherestart/fixture_cache_probe.php');
$t->assertSame('a script compiled after the restart is cached', $cached['cached'] ?? null, true);

$t->assertTrue('both pool threads run tasks with the cache again', ox_poll_tasks(true, 10.0));

$t->done();
