<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/../opcacherestart/lib.php';

$t = new TestCase('worker_reset_waits_for_this_request', 'opcacherestartworker');

// A worker opens its PHP request once, at boot, so a worker that booted with the
// cache keeps running code from OPcache's shared memory until it exits: a
// restart has to wait for every such worker to be replaced. This request holds
// the restart for as long as it runs.
//
// The restart is scheduled by a request this one sends, which the other worker
// serves: this one is busy with this request. The server acts on a restart once
// the request that scheduled it has closed, and a worker's request closes only
// when the worker exits, so the other worker retires, and its replacement —
// booted while this request holds the restart — runs without the cache; once
// the restart has run, that one is replaced again (opcacherestartworker suite,
// the blocks below).
//
// A server that leaves its workers alone never replaces the other worker: every
// request below still reports the cache, and the restart waits for good.
$before = ox_opcache_state();
$t->assertSame('premise: this worker runs with the cache', $before['opcache_enabled'], true);

$reset = ox_get_json('/tests/opcacherestart/fixture_reset.php');
$t->assertSame('opcache_reset() scheduled a restart', $reset['reset'] ?? null, true);

$other = ox_poll_state(static fn (array $s): bool => $s['opcache_enabled'] === false, 10.0);
$t->assertSame('the other worker was replaced by one without the cache', $other['opcache_enabled'] ?? null, false);
$t->assertSame('which sees the restart pending', $other['restart_pending'] ?? null, true);

$mine = ox_opcache_state();
$t->assertSame('the restart has not run under this request', $mine['manual_restarts'], $before['manual_restarts']);
$t->assertSame('the restart is still pending', $mine['restart_pending'], true);

$t->done();
