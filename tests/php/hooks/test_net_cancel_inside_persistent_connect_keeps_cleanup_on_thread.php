<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('net_cancel_inside_persistent_connect_keeps_cleanup_on_thread', 'hooks');

// A persistent connect keeps the thread while it connects, so the net category
// refuses to park inside it for exactly that long. A suspension that is not one of
// the connect's own waits can still happen in there, though: a name that does not
// resolve is reported as a warning before anything is connected, and an error
// handler that waits — to write its log to a socket, say — suspends the fiber, and
// is the one that can be handed a cancellation. Once it has been, what runs next is
// cleanup, and the cleanup must stay on the thread; the connect's own bookkeeping
// ending a moment later must not put the fiber's waits back.
//
// The task's error handler sleeps through the warning, is cancelled there when its
// awaiter gives up, and from finally connects to an address that never answers with
// a one-second timeout. That connect must come back with its own failure after that
// second, not with the cancellation a second time within milliseconds.
$silent = net_silent_address(6);
$marker = sys_get_temp_dir() . '/oxphp_netcancelpersist_' . getmypid() . '_' . uniqid('', true);

$task = oxphp_async(function (string $marker, string $silent): int {
    $report = [];
    set_error_handler(static function (): bool {
        oxphp_sleep(5.0);
        return true;
    });
    try {
        // A name with a space in it never resolves, and says so at once.
        pfsockopen('no such host.invalid', 80, $errno, $errstr, 1.0);
        $report['first'] = 'returned';
    } catch (\Throwable $e) {
        $report['first'] = get_class($e);
    } finally {
        restore_error_handler();
        $started = microtime(true);
        try {
            @stream_socket_client("tcp://{$silent}:80", $errno, $errstr, 1.0);
            $report['outcome'] = 'returned';
        } catch (\Throwable $e) {
            $report['outcome'] = get_class($e);
        }
        $report['seconds'] = microtime(true) - $started;
        file_put_contents($marker, json_encode($report));
    }
    return 0;
}, $marker, $silent);

$timedOut = false;
try {
    oxphp_async_await($task, 0.3);
} catch (\OxPHP\Async\TimeoutException $e) {
    $timedOut = true;
}
$t->assertTrue('the outer await timed out, arming cancellation', $timedOut);

$deadline = microtime(true) + 4.0;
while (!file_exists($marker) && microtime(true) < $deadline) {
    usleep(20000);
}
$t->assertTrue('the task got through its finally block', file_exists($marker));

if (file_exists($marker)) {
    $report = json_decode((string) file_get_contents($marker), true);
    unlink($marker);
    $t->assertSame(
        'the cancellation was taken inside the persistent connect',
        $report['first'] ?? null,
        'OxPHP\\Async\\AsyncException'
    );
    $t->assertSame(
        'the connect made during cleanup was not handed the cancellation again',
        $report['outcome'] ?? null,
        'returned'
    );
    $t->assertGreaterThan(
        'and it ran its own timeout, on the thread, instead of being cut short',
        (float) ($report['seconds'] ?? 0.0),
        0.8
    );
}

$alive = oxphp_async(static fn(): string => 'alive');
$t->assertSame('the pool still runs tasks', oxphp_async_await($alive, 3.0), 'alive');

$t->done();
