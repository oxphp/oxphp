<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A fatal error raised inside await_all() or await_any() while it releases a
// promise's closure ends that request, and only that one: the request does not
// park on the promise still outstanding, so the request this worker runs
// beside it never meets the fatal.

$t = new TestCase('await_all_no_park_after_held_back_bailout', 'fibers');

/** Sends one request to this server and returns the socket to read it from. */
$send = static function (string $query) {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return null;
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_held_back_bailout_park.php?$query HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    return $sock;
};

/** Reads a request sent with $send to its end. */
$read = static function ($sock): string {
    if ($sock === null) {
        return 'connect failed';
    }
    // Hooked (this profile hooks streams): the read parks this request's fiber,
    // which leaves the worker free to serve the requests just sent.
    $resp = (string) stream_get_contents($sock);
    fclose($sock);

    return $resp;
};

foreach (['await_all', 'await_any'] as $case) {
    // The other request first, so it is already running when this one parks
    // or does not.
    $beside = $send('case=spin');
    usleep(50_000);
    $failing = $send("case=$case");

    $resp = $read($failing);
    $t->assertMatch("$case: the fatal ended its request with a 500", $resp, '#^HTTP/\S+ 500 #');
    $t->assertNotContains("$case: nothing past the fatal ran", $resp, 'NOT-REACHED');

    $resp = $read($beside);
    $t->assertMatch("$case: the request beside it completed", $resp, '#^HTTP/\S+ 200 #');
    $t->assertContains("$case: the request beside it ran to its end", $resp, 'SPUN');
}

$t->done();
