<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Follows fibers/test_memory_limit_holds_after_a_deep_fatal_on_its_own, which
// ran out of memory deep in a call stack with nothing else in flight on the
// worker. That request went through the other of the worker's two ways of
// taking a request, the one that resets the worker before it starts; what it
// left has to be put right all the same.
//
// The probe runs as an inner request while this one is parked, for the reason
// fibers/test_memory_limit_holds_after_a_deep_fatal gives, since it ends in a
// fatal error when all is well.

$t = new TestCase('memory_limit_holds_after_a_deep_fatal_on_its_own_probe', 'fibers');

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
$t->assertTrue('the probe reported the heap and the limit', isset($real[1], $limit[1]));

$t->assertTrue(
    'the allocator counts less than memory_limit as in use after the deep fatal',
    isset($real[1], $limit[1]) && (int) $real[1] <= (int) $limit[1]
);
$t->assertContains('the next request is refused more than the limit', $probe, 'last error: Allowed memory size');
$t->assertNotContains('rather than given it', $probe, 'NOT-ENFORCED');

$t->done();
