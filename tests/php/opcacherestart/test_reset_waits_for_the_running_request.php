<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/lib.php';

$t = new TestCase('reset_waits_for_the_running_request', 'opcacherestart');

// This request runs code from OPcache's shared memory — its own script and the
// helpers it includes — for its whole life. A restart that resets that memory
// while the request is still running pulls the code out from under it, so a
// restart opcache_reset() schedules from another request has to wait for this
// one. Meanwhile a request that starts runs without the cache, as under
// PHP-FPM, and its own startup must not run the restart either.
//
// OPcache counts the requests that use the shared memory with fcntl() read
// locks and a restart asks fcntl(F_GETLK) whether there are any; a process never
// sees its own locks there, and every PHP thread of the server is one process.
// Without the server holding the restart, the startup of the request below runs
// it on the spot: that request reports the restart done, and so does this one.
$before = ox_opcache_state();
$t->assertSame('premise: this request runs with the cache', $before['opcache_enabled'], true);
$t->assertSame('premise: no restart is pending yet', $before['restart_pending'], false);

$reset = ox_get_json('/tests/opcacherestart/fixture_reset.php');
$t->assertSame('opcache_reset() scheduled a restart', $reset['reset'] ?? null, true);

$other = ox_get_json('/tests/opcacherestart/fixture_status.php');
$t->assertSame('a request started meanwhile sees the restart pending', $other['restart_pending'] ?? null, true);
$t->assertSame('a request started meanwhile runs without the cache', $other['opcache_enabled'] ?? null, false);
$t->assertSame('a request started meanwhile did not run the restart', $other['manual_restarts'] ?? null, $before['manual_restarts']);

// A pool thread keeps the request it opened at boot, with the cache, for its
// whole life, so it holds the restart too. Each, idle, is replaced while this
// request still holds the restart, and its replacement runs its tasks without
// the cache.
$t->assertTrue('the pool threads were replaced and run without the cache', ox_poll_tasks(false, 5.0));

$mine = ox_opcache_state();
$t->assertSame('the restart has not run under this request', $mine['manual_restarts'], $before['manual_restarts']);
$t->assertSame('the restart is still pending', $mine['restart_pending'], true);
$t->meta('manual_restarts_before', $before['manual_restarts']);

$t->done();
