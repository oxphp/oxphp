<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// An output buffer belongs to the request that opened it, but the stack it sits
// on belongs to the worker thread. A request that suspends with one open leaves
// it standing there, so the request the worker serves in that window writes into
// it — its body never reaches its own response, and the buffer's owner gets it
// instead. What the owner buffered before suspending has to still be there when
// it resumes, too: that is what ob_get_clean() after a suspension returns.
//
// The inner request is what proves both halves — it reports the buffer level it
// found and writes a body of its own.

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$connected = $sock !== false;

// Opened before the suspension and closed after it, with a request served in
// between.
ob_start();
echo "OUTER-BUFFERED\n";
$levelBefore = ob_get_level();

if ($connected) {
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_ob_probe.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
}

// Hooked: parks this request's fiber so the worker is free to serve the inner
// one. Without the park there is no worker to serve it — this profile runs one.
sleep(1);

$resp = $connected ? (string) stream_get_contents($sock) : '';
if ($connected) {
    fclose($sock);
}

$levelAfter = ob_get_level();
$buffered = (string) ob_get_clean();

$t = new TestCase('ob_survives_suspend', 'fibers');

$t->assertTrue('inner self-request socket connected', $connected);

$t->assertSame(
    'the buffer this request opened is still this request\'s after the suspension',
    $levelAfter,
    $levelBefore
);

$t->assertContains(
    'what it buffered before suspending is still buffered, not sent',
    $buffered,
    'OUTER-BUFFERED'
);

$t->assertNotContains(
    'the request served in the window did not write into it',
    $buffered,
    'INNER-BODY'
);

$t->assertContains(
    'that request wrote into its own response instead',
    $resp,
    'INNER-BODY'
);

$t->assertContains(
    'and started with no buffer of anyone else\'s open',
    $resp,
    'INNER-OB-LEVEL:0'
);

$t->done();
