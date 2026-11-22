<?php

declare(strict_types=1);

// Channel::recv() without a timeout, in a destructor the worker runs between two
// slices of its requests — of what a task captured, released once a task a
// finished request left running has stopped — is refused with an exception, as
// it is where a request cannot park. A wait there holds the worker thread, and a
// request parked on that worker — this one — cannot run until the wait ends;
// where that request is the other end, the wait would never end.
//
// This request parks on its inner request, which dispatches such a task, and
// then on the destructor's marker. It runs again only once the destructor has
// returned.

require_once __DIR__ . '/../test_helper.php';

$catch = ($_GET['catch'] ?? '') === '1';
$t = new TestCase('channel_wait_in_a_destructor_between_requests_throws' . ($catch ? '' : '_uncaught'), 'fibers');

$run = bin2hex(random_bytes(6));
$base = sys_get_temp_dir() . '/oxphp-channel-left-' . $run;

$sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
$t->assertTrue('inner self-request socket connected', $sock !== false);
if ($sock !== false) {
    stream_set_timeout($sock, 20);
    fwrite($sock, "GET /tests/fibers/fixture_channel_wait_left_to_a_task.php?run=$run&catch=" . ($catch ? '1' : '0') . " HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $resp = (string) stream_get_contents($sock);
    fclose($sock);
    $t->assertContains('the inner request left a running task holding the object', $resp, 'DISPATCHED');
}

$settled = static fn (): bool => is_file("$base.ran") && ($catch ? file_get_contents("$base.ran") !== 'entered' : true);
for ($i = 0; $i < 100 && !$settled(); $i++) {
    usleep(50_000);
}
$outcome = is_file("$base.ran") ? (string) file_get_contents("$base.ran") : null;

if ($catch) {
    $t->assertSame(
        'recv() in the destructor is refused rather than waiting on the worker thread',
        $outcome,
        'OxPHP\Shared\SharedException: Channel::recv() found nothing to receive and cannot wait without a timeout where the code running cannot be suspended; use recvTimeout()'
    );
} else {
    $t->assertSame('the destructor ran', $outcome, 'entered');
}

// This request was parked on the worker while the destructor ran: getting here
// is the worker having come back from it.
$t->assertSame('a callback still runs on the worker', array_map(static fn (int $n): int => $n * 2, [1, 2]), [2, 4]);

foreach (glob("$base.*") ?: [] as $file) {
    unlink($file);
}

$t->done();
