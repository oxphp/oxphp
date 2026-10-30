<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('task_last_error_does_not_outlive_its_request', 'asyncnocache');

// Without OPcache, the warning a task raises names the memory of the request
// that dispatched it: the file of the task's code, and the message too when it
// is a literal of that code. The async thread must hold neither once the task
// has ended, since the request can end next and free them, and must not count
// the file from its own thread while the request counts it from its own.
//
// A task of this request sleeps on the async thread meanwhile, so the thread
// is never idle between the two requests below.
$t->assertSame('OPcache is off', (bool) ini_get('opcache.enable'), false);

$get = static fn (string $fixture): string|false => @file_get_contents(
    "http://127.0.0.1/tests/asyncnocache/$fixture.php",
    false,
    stream_context_create(['http' => ['timeout' => 10]]),
);

$sentinel = oxphp_async(static function (): array {
    $started = microtime(true);
    oxphp_sleep(4.0);

    return [$started, microtime(true)];
});

$raised = json_decode((string) $get('fixture_last_error_raise'), true);
$t->assertSame('the task of the first request raises its warning', $raised['message'] ?? null, 'oxphp-last-error-of-an-ended-request');
$t->assertMatch('the file its warning names is not counted', $raised['file'] ?? '', '/^string\(\d+\) ".*\/fixture_last_error_raise\.php" interned\n$/');

$read = json_decode((string) $get('fixture_last_error_read'), true);
[$started, $ended] = oxphp_async_await($sentinel);
$t->assertTrue('the next request reads it back', is_array($read) && array_key_exists('seen', $read));
$t->assertTrue(
    'the async thread is not idle in between',
    $started < ($read['read_at'] ?? 0.0) && ($read['read_at'] ?? INF) < $ended,
);
$seen = is_array($read) && array_key_exists('seen', $read) ? $read['seen'] : 'unread';
$t->meta('seen', $seen);
$t->assertNull('the task of the next request finds no error of the one that ended', $seen);

$t->done();
