<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Follows fibers/test_memory_limit_retires_a_worker_it_cannot_return_to. On
// PHP 8.5 that request left the worker holding more than memory_limit allows,
// in memory it still uses, so handing back what the fatal's backtrace took
// cannot bring the worker under its limit again. A worker over its limit
// enforces none, and while that memory stays held only a fresh worker has the
// limit back: the worker is replaced as soon as it has no request in flight,
// and this line runs on its replacement.
//
// Before PHP 8.5 the fatal takes no backtrace, the shutdown function keeps
// nothing, and the worker has no reason to go.

$t = new TestCase('memory_limit_retires_a_worker_it_cannot_return_to_probe', 'fibers');

$file = '/tmp/oxphp-deep-backtrace-stash';
$state = is_file($file) ? json_decode((string) file_get_contents($file), true) : null;
$t->assertTrue('the previous line left its state', is_array($state));
$t->meta('state', $state);

// What the rest stands on: the previous line ran out of memory, and on 8.5 its
// shutdown function was given the memory it kept.
$t->assertContains('the previous line ran out of memory', (string) ($state['last_error'] ?? ''), 'Allowed memory size');
$expectStashed = PHP_VERSION_ID >= 80500 ? 200 << 20 : 0;
$t->assertSame('its shutdown function kept what it asked for', $state['stashed'] ?? null, $expectStashed);

$replaced = OxPHP\Server\Worker::current()->startTime() > (float) ($state['worker_started'] ?? INF);
$t->assertSame('a worker started after it serves this request', $replaced, PHP_VERSION_ID >= 80500);
$t->assertTrue('and holds nothing the previous line kept', !isset($sharedState['deep_backtrace_stash']));

// The limit is enforced on the worker serving this request. The probe runs as
// an inner request while this one is parked, for the reason
// fibers/test_memory_limit_holds_after_a_deep_fatal gives.
$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
$probe = '';
if ($sock !== false) {
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_deep_backtrace_fatal.php?step=probe HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    sleep(1);
    $probe = (string) stream_get_contents($sock);
    fclose($sock);
}
$t->meta('probe response', $probe);

preg_match('/^real: (\d+)$/m', $probe, $real);
preg_match('/^limit: (\d+)$/m', $probe, $limit);
$t->assertTrue(
    'the allocator counts less than memory_limit as in use',
    isset($real[1], $limit[1]) && (int) $real[1] <= (int) $limit[1]
);
$t->assertContains('a request is refused more than the limit', $probe, 'last error: Allowed memory size');
$t->assertNotContains('rather than given it', $probe, 'NOT-ENFORCED');

$t->done();
