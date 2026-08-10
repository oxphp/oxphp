<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// What a body parse is allowed to take down with it: its own request, and
// nothing else.
//
// The error handler installed here throws, which is what a strict-mode handler
// does for every warning — the harness's own does it, and so does every
// application that turns warnings into exceptions. The parse of an oversized
// body therefore ends in an exception raised from wherever that parse runs.
//
// Raised inside the request's fiber, it is an ordinary uncaught exception: the
// request answers 500, the server reports it, and the worker carries on. Raised
// on the worker's own stack, before the request has a fiber to run in, it is
// nobody's: the handler call the worker makes next returns on sight of a pending
// exception without ever entering the application, and the client is left with a
// connection that closes having said nothing at all.
//
// The second half of the test is the request parked below. It has nothing to do
// with the body that was oversized, and must not be able to tell that anything
// happened. PHP_WORKERS=1 (this profile) is what puts the two on one worker and
// makes the inner request land inside this one's suspension.

$t = new TestCase('parse_throw_ends_only_its_request', 'hooks');

set_error_handler(function (int $type, string $message): bool {
    // Narrow on purpose: this handler is thread-wide and outlives the request
    // that installs it, so throwing for everything would put this request's
    // exception into whatever else the worker warns about meanwhile.
    if (str_contains($message, 'file uploads')) {
        throw new \RuntimeException('parse-diag-boom-4f21');
    }
    return true;
});

// 21 files against a max_file_uploads of 20 — one warning, into the handler
// above, from a request that is not this one.
$boundary = '----oxphpParseThrowBoundary3b95';
$body = '';
for ($i = 0; $i < 21; $i++) {
    $body .= "--{$boundary}\r\n"
        . "Content-Disposition: form-data; name=\"f{$i}\"; filename=\"f{$i}.txt\"\r\n"
        . "Content-Type: text/plain\r\n\r\n"
        . "upload-{$i}\r\n";
}
$body .= "--{$boundary}--\r\n";

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
stream_set_timeout($sock, 5);
fwrite($sock, "POST /tests/hooks/fixture_inner_overlimit.php HTTP/1.0\r\n"
    . "Host: 127.0.0.1\r\n"
    . "Content-Type: multipart/form-data; boundary={$boundary}\r\n"
    . 'Content-Length: ' . strlen($body) . "\r\n"
    . "Connection: close\r\n\r\n"
    . $body);

sleep(2);                                   // hooked: suspends this request fiber

$resp = (string) stream_get_contents($sock);
fclose($sock);

// Back to the harness's own handler before anything below can warn.
restore_error_handler();

// The inner request answered, and answered a failure. What it must not be is
// silent: an empty response is what a request gets when the exception its parse
// raised belongs to no request at all.
$t->assertContains('the inner request answered at all', $resp, 'HTTP/1.');
$t->assertContains('and answered a server error', $resp, ' 500 ');
$t->assertNotContains('its handler never ran', $resp, 'INNER-OK');

// The point of the test. This request was parked in an unrelated sleep while
// somebody else's body was being parsed, and it is still here, still itself.
$t->assertNotNull('this request survived the inner failure', \Fiber::getCurrent());
$t->assertContains(
    'and came back to its own $_SERVER',
    $_SERVER['REQUEST_URI'],
    'test_parse_throw_ends_only_its_request.php'
);
$t->assertSame('and to its own $_GET', $_GET['probe'] ?? null, 'outer');

$t->done();
