<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A fatal raised inside a Fiber the request started — directly, or through a
// library that runs code in fibers of its own — ends the request, not the
// server.
//
// The engine destroys the fiber's VM stack before it hands the bailout to the
// request, so the frames the fatal was raised in are gone by the time the worker
// gives back what the request was holding. Following them read freed memory,
// and when the fiber had grown its stack, as a recursion that runs out of memory
// does, that memory was no longer mapped: the whole process went down with
// every connection it held. What the worker can still give back is the
// request's own frames below the point that started the fiber, and the inner
// request holds an object there to show it does.
//
// Two inner requests run on this worker while this one waits; both fatal, and
// the worker still serving this request afterwards is what says it survived
// them.

$t = new TestCase('fatal_inside_a_userland_fiber_keeps_the_worker', 'fibers');

foreach (['fiber' => 'one fiber deep', 'nested' => 'two fibers deep'] as $in => $label) {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $t->assertTrue("$label: inner self-request socket connected", $sock !== false);
    if ($sock === false) {
        continue;
    }
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_userland_fiber_fatal.php?in=$in HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");

    // Hooked: parks this request's fiber so the worker is free to serve the
    // inner one. Without the park there is no worker to serve it — this profile
    // runs one.
    sleep(1);

    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $t->meta("$in response", $resp);

    // The recursion inside the fiber is what fataled, and nothing after it ran:
    // without these the last assertion proves nothing, since a fixture that
    // returned normally would leave no abandoned frames to give back.
    $t->assertContains("$label: the fiber ran out of memory", $resp, 'last error: Allowed memory size');
    $t->assertNotContains("$label: and the request did not get past it", $resp, 'NOT-REACHED');

    // Given back by the worker, from the frame that started the fiber down.
    $t->assertContains(
        "$label: what the request held below the fiber did not outlive it",
        $resp,
        'HELD-FREED'
    );
}

// The inner requests ran on this same worker, so reaching here at all says
// their fatals left it in one piece.
$t->assertTrue('the worker still serves this request', oxphp_is_worker());

$t->done();
