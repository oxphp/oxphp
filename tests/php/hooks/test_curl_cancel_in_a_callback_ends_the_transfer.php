<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/net_helper.php';

$t = new TestCase('curl_cancel_in_a_callback_ends_the_transfer', 'hooks');

// A cancellation can reach a task inside curl_exec() while one of the transfer's
// callbacks waits rather than libcurl: a progress callback that sleeps, here. The
// sleep throws it, the callback lets it through, and libcurl, which goes on with the
// transfer after the callback returns, is told its next wait failed, so the task
// unwinds well before the three-second answer instead of waiting it out on the
// thread. PHP 8.6's ext/curl ends the transfer itself when a callback throws. The
// handle is then usable, as after a cancellation libcurl itself waited in.
$peer = net_peer_ip();
$marker = sys_get_temp_dir() . '/oxphp_curlcbcancel_' . getmypid() . '_' . uniqid('', true);

$started = microtime(true);
$task = oxphp_async(function (string $marker) use ($peer): int {
    $ch = curl_init("http://{$peer}:8080/3000/cancelled");
    $state = ['slept' => false, 'cb_cancelled' => false];
    curl_setopt_array($ch, [
        CURLOPT_RETURNTRANSFER => true,
        CURLOPT_TIMEOUT => 8,
        CURLOPT_NOPROGRESS => false,
        CURLOPT_XFERINFOFUNCTION => static function () use (&$state): int {
            if (!$state['slept']) {
                $state['slept'] = true;
                try {
                    usleep(500_000);
                } catch (\OxPHP\Async\AsyncException $e) {
                    $state['cb_cancelled'] = true;
                    throw $e;
                }
            }
            return 0;
        },
    ]);
    $first = null;
    try {
        $first = curl_exec($ch);
    } finally {
        $errno = curl_errno($ch);
        curl_setopt_array($ch, [
            CURLOPT_URL => "http://{$peer}:8080/0/again",
            CURLOPT_NOPROGRESS => true,
        ]);
        $again = curl_exec($ch);
        file_put_contents($marker, json_encode([
            'first' => $first,
            'errno' => $errno,
            'cb_cancelled' => $state['cb_cancelled'],
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

$deadline = microtime(true) + 5.0;
while (!file_exists($marker) && microtime(true) < $deadline) {
    usleep(20000);
}
$unwound = microtime(true) - $started;

$seen = file_exists($marker) ? json_decode((string) file_get_contents($marker), true) : null;
$t->assertTrue('the task finished', is_array($seen));
$t->assertSame('the cancellation was thrown in the sleeping callback', $seen['cb_cancelled'] ?? null, true);
$t->assertLessThan('the task unwound well before the three-second answer', $unwound, 1.5);
$t->assertTrue(
    'curl_exec() ended in the cancellation, not with a result',
    is_array($seen) && array_key_exists('first', $seen) && $seen['first'] === null
);
if (PHP_VERSION_ID >= 80600) {
    // ext/curl/interface.c curl_xferinfo(), 8.6.0: a callback that threw reports an abort.
    $t->assertSame('ext/curl ended the transfer on the exception (CURLE_ABORTED_BY_CALLBACK)', $seen['errno'] ?? null, CURLE_ABORTED_BY_CALLBACK);
} else {
    $t->assertSame('libcurl was told its next wait failed (CURLE_BAD_FUNCTION_ARGUMENT)', $seen['errno'] ?? null, CURLE_BAD_FUNCTION_ARGUMENT);
}
$t->assertSame('and the same handle ran the next transfer', $seen['again'] ?? null, 'curl-done:/0/again');
if (file_exists($marker)) {
    unlink($marker);
}

$t->done();
