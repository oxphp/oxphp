<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A request that runs out of memory deep in a call stack leaves the worker with
// memory_limit still enforced, for the requests it serves next.
//
// From PHP 8.5 the fatal's backtrace is taken past the limit, as the fixture
// describes. The engine hands that memory back to its allocator when it shuts a
// request down, and a worker does not shut its requests down; left alone, the
// allocator went on counting it as in use, and while it counted more than the
// limit it enforced no limit at all — the next request that ran away was ended
// by the kernel, with the whole server.
//
// Both inner requests run while this one is parked, so the worker always has a
// request in flight in between: it cannot wait for a moment with nothing to
// serve to put things right, it has to do it as the request that fataled ends.

$t = new TestCase('memory_limit_holds_after_a_deep_fatal', 'fibers');

$inner = static function (string $step) use ($t): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue("$step: inner self-request socket connected", $sock !== false);
    if ($sock === false) {
        return '';
    }
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_deep_backtrace_fatal.php?step=$step HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    // Hooked: parks this request's fiber so the worker is free to serve the
    // inner one. Without the park there is no worker to serve it — this profile
    // runs one.
    sleep(1);

    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $t->meta("$step response", $resp);

    return $resp;
};

$fatal = $inner('fatal');

// The deep fatal happened, and is what ended that request.
$t->assertContains('the deep request ran out of memory', $fatal, 'last error: Allowed memory size');
$t->assertNotContains('and did not get past it', $fatal, 'NOT-REACHED');

$probe = $inner('probe');

preg_match('/^real: (\d+)$/m', $probe, $real);
preg_match('/^limit: (\d+)$/m', $probe, $limit);
$t->assertTrue('the probe reported the heap and the limit', isset($real[1], $limit[1]));

// Both halves: what the allocator counts as in use is back under the limit, and
// the limit is enforced again — a request that asks for more than it allows is
// refused rather than given the memory.
$t->assertTrue(
    'the allocator counts less than memory_limit as in use after the deep fatal',
    isset($real[1], $limit[1]) && (int) $real[1] <= (int) $limit[1]
);
$t->assertContains('the next request is refused more than the limit', $probe, 'last error: Allowed memory size');
$t->assertNotContains('rather than given it', $probe, 'NOT-ENFORCED');

// Both inner requests ran on the worker serving this one, not on a replacement:
// one worker, and it was never without a request in flight in between.
$started = sprintf('%.6f', OxPHP\Server\Worker::current()->startTime());
$t->assertContains('the deep fatal ran on this worker', $fatal, "worker started: $started");
$t->assertContains('and so did the probe', $probe, "worker started: $started");

$t->done();
