<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// Same check as fibers/test_gc_survives_fatal, from the one place a worker runs
// PHP with nothing of its own behind it: a shutdown function. The engine calls
// those under a try of its own that swallows the bailout, so the worker is
// handed a request that looks like it ended normally while everything the fatal
// left behind is still in place — the abandoned frames, the stack top inside
// them, the cleared execution cursor and both raised flags. The collector's flag
// is the one this test can see from userland.
//
// The fatal comes from the inner request — a request cannot report on its own —
// and this one checks the collector afterwards, on the same worker thread.

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$connected = $sock !== false;

if ($connected) {
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_shutdown_fatal.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
}

// Hooked: parks this request's fiber so the worker is free to serve the inner
// one. Without the park there is no worker to serve it — this profile runs one.
sleep(1);

$resp = $connected ? (string) stream_get_contents($sock) : '';
if ($connected) {
    fclose($sock);
}

// Anything this request buffered as a possible root before the window is still
// in the collector's buffer; drained first so the count below can only come from
// the cycle built after it.
gc_collect_cycles();

$cycle = new \stdClass();
$cycle->self = $cycle;
unset($cycle);

$collected = gc_collect_cycles();

$t = new TestCase('gc_survives_shutdown_fatal', 'fibers');

$t->assertTrue('inner self-request socket connected', $connected);
$t->assertTrue('the collector is enabled at all', gc_enabled());

// The flag is only raised by a fatal that actually happened, so an inner request
// the worker never got to serve would pass the check below on its own. Both
// halves are asserted: the request ran, and its shutdown function fataled.
$t->assertContains('the inner request armed its shutdown fatal', $resp, 'SHUTDOWN-FATAL-ARMED');
$t->assertContains('and took it', $resp, 'fatal from a shutdown function');

$t->assertGreaterThan(
    'the worker still collects cycles after a fatal in a shutdown function',
    $collected,
    0
);

$t->done();
