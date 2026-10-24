<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A request that runs past memory_limit as the end of the request writes its
// session through the files handler is the request filed for it, and the
// requests after it on the worker are not handed the error.
//
// From PHP 8.4.27 / 8.5.12 the engine lets such an allocation through and
// reports the error at the next point that checks for it. The files handler and
// the serialiser it writes are C code with no such point, so the request ended
// with the error still unreported, and it came out in whatever ran next on the
// worker — this request, as it went on after the read below, or the probe.
//
// Both inner requests run while this one is parked, so they land on the worker
// serving it, one after the other. This request getting as far as its
// assertions is part of what is checked.

$t = new TestCase('oom_in_the_session_write_stays_with_its_request', 'fibers');

$inner = static function (string $step) use ($t): string {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue("$step: inner self-request socket connected", $sock !== false);
    if ($sock === false) {
        return '';
    }
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_oom_in_the_session_write.php?step=$step HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $t->meta("$step response", $resp);

    return $resp;
};

$oom = $inner('oom');
$probe = $inner('probe');

$t->assertContains('the request that ran out ran its handler', $oom, 'SESSION-SET');
$t->assertContains('and is the one that fails', $oom, 'HTTP/1.0 500 ');
// It set display_errors off, and the error is reported while that still holds,
// not once the worker's own settings are back.
$t->assertNotContains('under the display_errors it set', $oom, 'Allowed memory size');

$t->assertContains('the next request runs', $probe, 'PROBE-RAN');
$t->assertNotContains('without the error', $probe, 'Allowed memory size');
$started = sprintf('%.6f', OxPHP\Server\Worker::current()->startTime());
$t->assertContains('on the worker serving this request', $probe, "worker started: $started");

$t->done();
