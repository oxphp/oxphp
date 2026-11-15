<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_cancel_hooked', 'hooks');

// The scheduler's cancellation reaches a task parked inside curl_exec(): the task
// unwinds when its awaiter gives up instead of waiting out a five-second answer.
// libcurl is told its wait failed, so curl_exec() ends with
// CURLE_BAD_FUNCTION_ARGUMENT and the cancellation is thrown from it, and the
// handle is left usable: the finally block runs a second transfer on it, which a
// handle left mid-call inside libcurl would refuse.
$peer = net_peer_ip();
$marker = sys_get_temp_dir() . '/oxphp_curlcancel_' . getmypid() . '_' . uniqid('', true);

$started = microtime(true);
$task = oxphp_async(function (string $marker) use ($peer): int {
    $ch = curl_init("http://{$peer}:8080/5000/cancelled");
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 8]);
    $first = null;
    try {
        $first = curl_exec($ch);
    } finally {
        $errno = curl_errno($ch);
        curl_setopt($ch, CURLOPT_URL, "http://{$peer}:8080/0/again");
        $again = curl_exec($ch);
        file_put_contents($marker, json_encode([
            'first' => $first,
            'errno' => $errno,
            'again' => $again,
        ]));
    }
    return 0;
}, $marker);

$timedOut = false;
try {
    oxphp_async_await($task, 0.2);
} catch (\OxPHP\Async\TimeoutException $e) {
    $timedOut = true;
}
$t->assertTrue('the outer await timed out, arming cancellation', $timedOut);

$deadline = microtime(true) + 3.0;
while (!file_exists($marker) && microtime(true) < $deadline) {
    usleep(20000);
}
$unwound = microtime(true) - $started;

$t->assertTrue('the task parked in curl_exec() was cancelled and unwound', file_exists($marker));
$t->assertLessThan('well before the five-second answer', $unwound, 1.5);

$seen = file_exists($marker) ? json_decode((string) file_get_contents($marker), true) : null;
$t->assertTrue(
    'curl_exec() ended in the cancellation, not with a result',
    is_array($seen) && array_key_exists('first', $seen) && $seen['first'] === null
);
$t->assertSame('libcurl was told the wait failed (CURLE_BAD_FUNCTION_ARGUMENT)', $seen['errno'] ?? null, CURLE_BAD_FUNCTION_ARGUMENT);
$t->assertSame('and the same handle ran the next transfer', $seen['again'] ?? null, 'curl-done:/0/again');
if (file_exists($marker)) {
    unlink($marker);
}

// Cancelling it did not cost the pool its workers.
$alive = oxphp_async(static fn(): string => 'alive');
$t->assertSame('the pool still runs tasks', oxphp_async_await($alive, 3.0), 'alive');

$t->done();
