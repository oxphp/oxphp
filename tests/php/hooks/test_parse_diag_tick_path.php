<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// The other dispatch path, same guarantee.
//
// A worker builds a request's input in one of two places: the fast path, when
// nothing is suspended and the request is handed straight to a fiber, and the
// event-loop tick, when something is. hooks/test_parse_diag_sees_its_own_request
// covers the first. This one covers the second, by parking in a hooked sleep()
// and having the inner self-request served from inside that window.
//
// PHP_WORKERS=1 (this profile), so the inner request can only be served while
// this fiber is parked — and the error handler this request installs is the one
// its parse will call, because the handler slot is thread-wide and worker mode
// never clears it.

$t = new TestCase('parse_diag_tick_path', 'hooks');

$GLOBALS['ox_parse_diag'] = [];
set_error_handler(function (int $type, string $message): bool {
    // See hooks/test_parse_diag_sees_its_own_request for what this asks and why.
    // It matters more here than there: on this path a sibling request really is
    // parked on the same worker, so a parse that could park would be handing it
    // the fields it is halfway through.
    $switchBlocked = false;
    try {
        (new \Fiber(static function (): void {
        }))->start();
    } catch (\FiberError) {
        $switchBlocked = true;
    }

    $GLOBALS['ox_parse_diag'][] = [
        'msg' => $message,
        'uri' => $_SERVER['REQUEST_URI'] ?? '(no $_SERVER)',
        'fiber' => \Fiber::getCurrent() !== null,
        'switch_blocked' => $switchBlocked,
    ];
    return true;
});

// 21 files against a max_file_uploads of 20: one warning, raised from the
// multipart parse of a request that is not this one.
$boundary = '----oxphpTickPathBoundary7e04';
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

// Back to the throwing handler the harness expects, so that a warning raised by
// anything below is reported as one rather than quietly recorded.
restore_error_handler();

// Without this the rest proves nothing: an inner request that was not served
// inside the window never had its body parsed under this handler at all.
$t->assertContains('the inner request was served while this one was parked', $resp, 'INNER-OK');
$t->assertContains('and its parse kept the uploads below the limit', $resp, 'files=20');

$log = $GLOBALS['ox_parse_diag'] ?? null;
$t->assertTrue('the inner parse called the error handler', is_array($log) && $log !== []);

$first = is_array($log) && $log !== [] ? $log[0] : [];
$t->assertContains(
    'the handler was told about the upload limit',
    (string) ($first['msg'] ?? ''),
    'Maximum number of allowable file uploads has been exceeded'
);

// The discriminating checks, as on the fast path: a parse that runs on the
// worker's own stack reports the request that is parked, not the one being read.
$t->assertContains(
    'the handler saw the INNER request in $_SERVER',
    (string) ($first['uri'] ?? ''),
    'fixture_inner_overlimit.php'
);
$t->assertTrue('the handler ran inside a request fiber', ($first['fiber'] ?? null) === true);
$t->assertTrue(
    'and the parse held fiber switching down while it ran',
    ($first['switch_blocked'] ?? null) === true
);

// And this request came back to its own.
$t->assertContains('this request still has its own $_SERVER', $_SERVER['REQUEST_URI'], 'test_parse_diag_tick_path.php');

$t->done();
