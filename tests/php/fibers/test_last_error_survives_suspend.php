<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// error_get_last() reads state the engine keeps on the worker thread, not on the
// request. Both directions of that matter, and both are checked here from one
// request that raises an error, parks, and lets another request raise its own
// inside the window:
//
//   - the request served in the window must not start able to read this one's
//     error, and must not leave its own where this one will read it on resume;
//   - this one must come back reading exactly what it raised before parking.
//
// Where this shows up in practice is a shutdown function: reading
// error_get_last() there to decide whether the request being closed died on a
// fatal is how frameworks catch fatals at all, and reading another request's
// answer makes them report a failure for a request that succeeded.

// Cleared so the error below is recorded as one rather than turned into an
// exception by whichever handler the last request on this worker installed.
set_error_handler(null);
set_exception_handler(null);

// Silenced, not unreported: @ only stops the display, and displaying it here
// would send this response's headers long before the test writes its own.
// error_get_last() records it either way.
@trigger_error('outer request error', E_USER_WARNING);
$beforeSuspend = error_get_last();

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$connected = $sock !== false;

if ($connected) {
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_error_probe.php HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
}

// Hooked: parks this request's fiber so the worker is free to serve the inner
// one. Without the park there is no worker to serve it — this profile runs one.
sleep(1);

$afterResume = error_get_last();

$resp = $connected ? (string) stream_get_contents($sock) : '';
if ($connected) {
    fclose($sock);
}

$t = new TestCase('last_error_survives_suspend', 'fibers');

$t->assertTrue('inner self-request socket connected', $connected);
$t->assertContains('the inner request ran and raised its own error', $resp, 'INNER-ERROR-RAISED');

$t->assertSame(
    'this request records its own error before suspending',
    $beforeSuspend['message'] ?? null,
    'outer request error'
);

$t->assertContains(
    'the request served in the window starts with no last error of its own to read',
    $resp,
    'INNER-LAST-ERROR-AT-START:none'
);

$t->assertSame(
    'and this one resumes still reading the error it raised itself',
    $afterResume['message'] ?? null,
    'outer request error'
);

$t->done();
