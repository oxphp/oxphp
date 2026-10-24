<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A request that fatals, and then runs out of memory in the userland the
// worker's recovery from that fatal reaches — a stream filter's onClose(), as
// the recovery gives back the stream the fatal's frame held — still runs its
// shutdown functions, as a request that only fatals does.
//
// From PHP 8.4.27 / 8.5.12 the engine reports running out of memory from a point
// where it can unwind the code that did, and keeps a note that it has until the
// request is shut down; while the note stands it calls no callback and runs no
// destructor. The recovery cleared the note before running that userland and
// not after, so the shutdown functions met it: on PHP 8.4 they were skipped
// without a word.
//
// The inner request runs while this one is parked, so it lands on the worker
// serving it.

$t = new TestCase('oom_in_a_filter_close_after_a_fatal_runs_the_shutdown_functions', 'fibers');

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
if ($sock !== false) {
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_oom_in_a_filter_close_after_a_fatal.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    // Hooked: parks this request's fiber so the worker is free to serve the
    // inner one. Without the park there is no worker to serve it — this profile
    // runs one.
    sleep(1);

    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $t->meta('response', $resp);

    $t->assertContains('the request answers 500', $resp, ' 500 Internal Server Error');
    // What the rest stands on: the recovery reached the filter.
    $t->assertContains('the recovery closed the stream and called onClose()', $resp, 'ONCLOSE-RAN');
    // Both halves at once: the shutdown function ran, and it ran after onClose()
    // had run out of memory — nothing else in the request allocates past the
    // limit, and the fatal before it has a message of its own.
    $t->assertContains(
        'its shutdown function runs, after onClose() ran out of memory',
        $resp,
        'SHUTDOWN-RAN: Allowed memory size'
    );
}

$t->done();
