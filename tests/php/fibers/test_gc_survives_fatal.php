<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A bailout raises two flags, not one. The second is the cycle collector's, and
// the engine lowers it where it lowers everything else a request starts with —
// once per PHP request, which in a worker is once per worker. So after the first
// fatal nothing is ever buffered as a possible root again, and every cycle any
// later request builds on this worker lives as long as the worker does: a
// container holding the services holding it, a parent holding its children, a
// closure bound to the object that stores it.
//
// The fatal comes from the inner request — a request cannot report on its own —
// and this one checks the collector afterwards, on the same worker thread.

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$connected = $sock !== false;

if ($connected) {
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_plain_fatal.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
}

// Hooked: parks this request's fiber so the worker is free to serve the inner
// one. Without the park there is no worker to serve it — this profile runs one.
sleep(1);

$resp = $connected ? (string) stream_get_contents($sock) : '';
if ($connected) {
    fclose($sock);
}

// Whatever this request buffered as a possible root before the fatal is still in
// the collector's buffer and would be collected by the call below whether the
// flag is up or not. Drained first, so the count that matters can only come from
// the cycle built after it.
gc_collect_cycles();

$cycle = new \stdClass();
$cycle->self = $cycle;
unset($cycle);

$collected = gc_collect_cycles();

$t = new TestCase('gc_survives_fatal', 'fibers');

$t->assertTrue('inner self-request socket connected', $connected);
$t->assertTrue('the collector is enabled at all', gc_enabled());

// Without this the rest proves nothing, and an inner request the worker never
// got to serve would prove it just as well: the flag is only raised by a fatal
// that actually happened, so the check below has to see the fatal itself.
$t->assertContains('the inner request took its fatal', $resp, 'gc probe fatal');
$t->assertNotContains('and did not reach the end of the script', $resp, 'NOT-REACHED');

$t->assertGreaterThan(
    'the worker still collects cycles after a fatal',
    $collected,
    0
);

$t->done();
