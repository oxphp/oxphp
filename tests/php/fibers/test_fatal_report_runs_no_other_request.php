<?php

declare(strict_types=1);

// An object made by a request beside one that fails keeps its destructor even
// when the failing request displays its fatal through an output handler that
// sleeps. The engine flags every object on the thread as destructed only after
// that display; the worker remembers which ones were not before it begins and
// takes the flag back off them once the failed request is let go of. A sleep in
// the handler that parked the failing request would let this one run in between
// and make an object neither list holds, so the display does not park: the
// handler sleeps, and this request waits for it.
//
// This request sends the inner one and polls for the mark its handler leaves on
// starting, then makes its object and reads the inner request's answer.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/destruct_probe.php';

$t = new TestCase('fatal_report_runs_no_other_request', 'fibers');

OxphpDestructProbe::reset(['late', 'failed', 'report_started', 'report_done']);

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 5.0);
if ($sock === false) {
    throw new \RuntimeException("inner connect failed: $errstr ($errno)");
}
stream_set_timeout($sock, 10);
fwrite($sock, "GET /tests/fibers/fixture_fatal_beside_a_parked_request.php?handler=1 HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");

$deadline = microtime(true) + 5.0;
while (!OxphpDestructProbe::destructed('report_started') && microtime(true) < $deadline) {
    usleep(10_000);
}
$displayDone = OxphpDestructProbe::destructed('report_done');
$late = new OxphpDestructProbe('late');

$raw = (string) stream_get_contents($sock);
fclose($sock);
unset($late);

$t->assertTrue('the inner request displayed its fatal through the handler', OxphpDestructProbe::destructed('report_started'));
$t->assertContains('and the display reached its client', $raw, 'OxphpDestructProbeFatal');
$t->assertTrue('this request ran only once that display was over', $displayDone);
$t->assertTrue("the destructor of this request's object ran", OxphpDestructProbe::destructed('late'));
$t->assertFalse("the failed request's object had no destructor run", OxphpDestructProbe::destructed('failed'));

$t->done();
