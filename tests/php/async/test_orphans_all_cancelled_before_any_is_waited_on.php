<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('orphans_all_cancelled_before_any_is_waited_on', 'async');

// Two orphans on a single pool thread: the first spins in a loop that never
// suspends, the second is queued behind it. The end of the request must cancel
// both before it waits for either.
//
// Waiting for one before cancelling the next fails whichever way round they are
// taken. Second first: it is still queued, so nothing can unwind it, and the
// wait for it sits behind the first, which nothing has cancelled yet — the
// response is held for the 5 s budget. First first: it unwinds, the pool thread
// picks up the second at once, and the second starts running before its cancel
// arrives. Cancelled up front, the second finds its cancel flag set when it is
// picked up and never starts.
//
// Needs ASYNC_WORKERS=1, so that the second orphan has to wait for the first,
// and PHP_WORKERS >= 2, so that the fixture request runs while this one waits.
$id = bin2hex(random_bytes(6));
$base = sys_get_temp_dir() . "/oxphp_orphan_{$id}";

$t0 = microtime(true);
$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('fixture socket connected', $sock !== false);
if ($sock === false) {
    $t->done();
}
stream_set_timeout($sock, 12);
fwrite($sock, "GET /tests/async/fixture_orphan_cpu_bound.php?id={$id}&orphans=2 HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
$resp = (string) stream_get_contents($sock);
fclose($sock);
$elapsed = microtime(true) - $t0;

$t->assertContains('the fixture returned once the first orphan was running', $resp, 'orphans-dispatched');

// The response is sent only after the end of the request has waited for both
// orphans, so what they recorded is final by now.
clearstatcache();
$first = file_exists("{$base}_0.result")
    ? (string) file_get_contents("{$base}_0.result")
    : '(no result)';
$t->assertSame(
    'the spinning orphan was unwound by the cancellation',
    $first,
    'OxPHP\Async\AsyncException: Async task cancelled'
);
$t->assertFalse('the queued orphan was cancelled before it started', file_exists("{$base}_1.started"));
$t->assertLessThan('the response did not wait out the 5 s budget for either orphan', $elapsed, 2.0);
$t->meta('elapsed', round($elapsed, 3));

foreach (glob("{$base}_*") ?: [] as $file) {
    unlink($file);
}
$t->done();
