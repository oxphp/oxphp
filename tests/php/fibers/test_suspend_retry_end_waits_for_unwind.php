<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('suspend_retry_end_waits_for_unwind', 'fibers');

// A request that keeps retrying Fiber::suspend() is ended with a graceful exit,
// and its unwind can suspend the fiber again: a finally block that parks, or one
// that throws to a catch which suspends once more. The fiber is still running
// that request either way, so the response waits for the unwind to finish, and
// the request after it on this worker runs its own handler rather than the rest
// of that one.
//
// PHP_WORKERS=1 and the socket reads below are hooked: the fixture runs while
// this request waits for its reply, and the request after it is given the fiber
// it ran on.
function fetch_unwind_fixture(string $query): string
{
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return '';
    }
    stream_set_timeout($sock, 10);
    fwrite($sock, "GET /tests/fibers/fixture_suspend_retry_unwind.php?{$query} HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $reply = (string) stream_get_contents($sock);
    fclose($sock);

    return $reply;
}

foreach (['park', 'escape'] as $mode) {
    $id = bin2hex(random_bytes(4));
    $marker = sys_get_temp_dir() . '/oxphp-req-suspend-' . $id;

    $reply = fetch_unwind_fixture("mode={$mode}&id={$id}");
    $finished = file_exists($marker);
    $t->assertTrue("{$mode}: the ended request answered", $reply !== '');
    $t->assertTrue("{$mode}: the answer came after its unwind finished", $finished);
    if ($finished) {
        unlink($marker);
    }

    $next = fetch_unwind_fixture('mode=plain');
    $t->assertContains("{$mode}: the next request runs its own handler", $next, 'handler ran to its end: plain');
}

$t->done();
