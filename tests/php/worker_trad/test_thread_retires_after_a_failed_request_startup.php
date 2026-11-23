<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// A request whose startup fails leaves the thread's PHP state as the startup
// left it: no php_request_shutdown runs for it, and what the startup had
// already set up — the session it was decoding, the instance a woken object
// asked Worker::current() for — stays behind. The thread does not serve
// another request on that state: it is replaced. Nor does it leave behind the
// POSIX timer PHP keeps for max_execution_time, which only that missing
// shutdown would have deleted.
//
// This test holds one of the profile's two workers, so the requests it sends
// run on the other.
$t = new TestCase('test_thread_retires_after_a_failed_request_startup', 'worker_trad');

$get = static function (string $path, ?string $cookie = null): array {
    $ch = curl_init('http://127.0.0.1' . $path);
    curl_setopt_array($ch, [CURLOPT_RETURNTRANSFER => true, CURLOPT_TIMEOUT => 10]);
    if ($cookie !== null) {
        curl_setopt($ch, CURLOPT_COOKIE, $cookie);
    }
    $body = curl_exec($ch);
    $status = (int) curl_getinfo($ch, CURLINFO_RESPONSE_CODE);
    $decoded = is_string($body) ? json_decode($body, true) : null;
    return [$status, is_array($decoded) ? $decoded : null, is_string($body) ? $body : ''];
};

// The process's POSIX timers. PHP keeps one per thread while the thread runs a
// request, and deletes it as the request shuts down.
$timers = static fn (): int => (int) preg_match_all('/^ID: /m', (string) @file_get_contents('/proc/self/timers'));

// Seed a session that cannot be decoded on the other worker, send its cookie,
// and wait until a request runs on a thread other than the one that failed.
$failOnce = static function () use ($get): array {
    [, $seed] = $get('/tests/worker_trad/fixture_autostart_fail_seed.php');
    $sid = $seed['sid'] ?? null;
    $thread = $seed['thread'] ?? null;
    [, $again] = $get('/tests/worker_trad/fixture_autostart_thread.php');
    [$failed, , $body] = $get(
        '/tests/worker_trad/fixture_autostart_thread.php',
        'PHPSESSID=' . (is_string($sid) ? $sid : '')
    );
    $deadline = microtime(true) + 5.0;
    do {
        [$status, $who] = $get('/tests/worker_trad/fixture_autostart_thread.php');
        if ($status === 200 && is_string($who['thread'] ?? null) && $who['thread'] !== $thread) {
            break;
        }
        usleep(100000);
    } while (microtime(true) < $deadline);
    return compact('seed', 'sid', 'thread', 'again', 'failed', 'body', 'status', 'who');
};

$timersBefore = $timers();
$t->assertTrue('premise: the process lists its POSIX timers, one of them this request\'s', $timersBefore >= 1);

$r = $failOnce();
$t->assertType('premise: the seeding request opened a session', $r['sid'], 'string');
$t->assertType('premise: it named the OS thread it ran on', $r['thread'], 'string');
$t->assertTrue(
    'premise: it ran on the other worker',
    is_int($r['seed']['worker'] ?? null) && $r['seed']['worker'] !== oxphp_worker_id()
);
$t->assertSame('premise: the next request ran on the same thread', $r['again']['thread'] ?? null, $r['thread']);
$t->assertSame('premise: the request whose session cannot be decoded failed to start', $r['failed'], 500);
$t->assertContains('premise: it failed in its startup', $r['body'], 'php_request_startup() failed');
$t->assertSame('the next request was served', $r['status'], 200);
$t->assertTrue(
    'it ran on a thread started in place of the one that failed',
    is_string($r['who']['thread'] ?? null) && $r['who']['thread'] !== $r['thread']
);
$t->meta('seeded_on', $r['seed']);
$t->meta('served_by', $r['who']);

// Two more, each with a session of its own: the file of one that failed stays
// locked, and a request that sent its cookie again would wait on that lock.
$failures = (int) ($r['failed'] === 500);
for ($i = 0; $i < 2; $i++) {
    $r = $failOnce();
    $failures += (int) ($r['failed'] === 500 && $r['status'] === 200 && ($r['who']['thread'] ?? null) !== $r['thread']);
}
$t->assertSame('premise: three requests failed to start, each on a thread that was then replaced', $failures, 3);
$timersAfter = $timers();
$t->meta('timers', ['before' => $timersBefore, 'after' => $timersAfter]);
// At most one more: the replacement's own, until the request it served last
// has shut down.
$t->assertTrue(
    'the threads that failed to start a request left no timer behind',
    $timersAfter - $timersBefore <= 1
);

$t->done();
