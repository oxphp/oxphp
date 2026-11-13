<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/argument_probe.php';

// A call that skips a parameter by naming a later one has the skipped default
// evaluated before it is entered, with the call standing as the current frame
// meanwhile — and linked back to the frame that makes it in place of the calls
// that frame had begun around it. A fatal in that evaluation leaves the worker
// a frame that was never entered: its arguments are set up, its variables,
// temporaries and pending calls are whatever the stack held — the inner request
// leaves the object there, references already given back. Released as an
// entered frame, those lose references they do not own; released again as the
// pending call the frame also is, so do the arguments, and the release runs on
// along the borrowed link into the calling frame itself. Through
// zend_call_function() the call is nobody's pending call, and its arguments are
// its own to give back.

OxphpArgumentProbe::$held ??= new stdClass();

$t = new TestCase('fatal_in_a_default_argument_leaves_the_call_alone', 'fibers');

foreach (['call', 'reflection'] as $via) {
    $before = OxphpArgumentProbe::refcount();

    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    $resp = null;
    if ($sock !== false) {
        stream_set_timeout($sock, 10);
        fwrite($sock, "GET /tests/fibers/fixture_default_argument_fatal.php?via={$via} HTTP/1.0\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n");
        // Hooked: parks this request while the worker serves the inner one.
        $resp = (string) stream_get_contents($sock);
        fclose($sock);
    }

    $after = OxphpArgumentProbe::refcount();

    $t->assertTrue("{$via}: the inner request connected", $resp !== null);
    $t->assertContains("{$via}: it fataled in the default's constructor", (string) $resp, 'OxphpDefaultArgumentFatal');
    $t->assertNotContains("{$via}: and nothing past the call ran", (string) $resp, 'NOT-REACHED');
    $t->assertTrue("{$via}: the object was counted before", $before > 0);
    $t->assertSame("{$via}: the object passed to the call keeps every reference it had", $after, $before);
}

$t->done();
