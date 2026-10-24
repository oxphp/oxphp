<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('shared_mysqli_close_under_unwinding_reader', 'hooksdb');

// test_shared_mysqli_close_under_parked_reader with the parked read made by a
// destructor while an exception unwinds — a statement's unbuffered result freed
// on the way out of the frame that threw. The wait sets that exception aside
// and puts it back when the fiber resumes, so it is pending again when the close
// is found and the request is ended with a fatal. The catch it was on its way to
// does not run then, and PHP reports a fatal's pending exception first, as
// uncaught: the log would blame the request's end on an exception its code
// catches. The bounds are the ones that test pins, and for the same reasons.
set_time_limit(30);

$key = bin2hex(random_bytes(6));
$log = "/tmp/oxphp-unwind-reader-{$key}.log";

$task = oxphp_async(function () use ($key): array {
    $sock = stream_socket_client('tcp://127.0.0.1:80', $errno, $errstr, 3.0);
    if ($sock === false) {
        return ['body' => "connect failed: {$errstr} ({$errno})"];
    }
    stream_set_timeout($sock, 15);
    fwrite($sock, "GET /tests/hooksdb/fixture_shared_mysqli_unwind_park.php?key={$key} HTTP/1.0\r\n"
        . "Host: 127.0.0.1\r\nConnection: close\r\n\r\n");
    $body = (string) stream_get_contents($sock);
    fclose($sock);

    return ['body' => $body];
});

$deadline = microtime(true) + 3.0;
while (!($sharedState['mysqli_unwind_doomed_parked'] ?? false) && microtime(true) < $deadline) {
    oxphp_usleep(20_000);
}

$built = isset($sharedState['mysqli_unwind_doomed']);
$parked = $built && ($sharedState['mysqli_unwind_doomed_parked'] ?? false);
$t->assertTrue('the holding request built the shared connection before this one woke', $built);
$t->assertTrue('and threw with the rest of its result still to come', $parked);
if (!$parked) {
    $t->done();
}

$started = microtime(true);
$sharedState['mysqli_unwind_doomed']->close();
unset($sharedState['mysqli_unwind_doomed'], $sharedState['mysqli_unwind_doomed_parked']);
$closeTook = microtime(true) - $started;

$inner = oxphp_async_await($task);
$waited = microtime(true) - $started;
$reported = is_file($log) ? (string) file_get_contents($log) : '';
@unlink($log);

$t->assertGreaterThan('close() waited for the holder before giving up', $closeTook, 1.5);
$t->assertLessThan('and gave up at the smaller of the two limits', $closeTook, 5.0);
$t->assertMatch(
    'the unwinding request was ended with a server error',
    $inner['body'],
    '#^HTTP/1\.[01] 500#'
);
$t->assertNotContains(
    'and its catch did not run on the connection closed under it',
    $inner['body'],
    'mysqli-unwind-'
);
$t->assertLessThan('it learned of the close rather than waiting out its query', $waited, 5.0);
$t->assertContains(
    'its log names the close as what ended it',
    $reported,
    'was reading from was closed by another fiber'
);
$t->assertNotContains(
    'and does not report the exception its catch was waiting for as uncaught',
    $reported,
    'Uncaught'
);

// The line it names is the call's: the destructor ran as the frame that threw was
// left, with the caller's frame current, so the throw's line would be another
// frame's line in this frame's file.
$callLine = 0;
foreach ((array) file(__DIR__ . '/fixture_shared_mysqli_unwind_park.php') as $i => $source) {
    if (str_contains((string) $source, '$read($sharedState')) {
        $callLine = $i + 1;
    }
}
preg_match('/was closed by another fiber.* on line (\d+)/', $reported, $line);
$t->assertSame('and names the line of the call the destructor ran under', $line[1] ?? 'none', (string) $callLine);

$t->done();
