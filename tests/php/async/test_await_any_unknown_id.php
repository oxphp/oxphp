<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('await_any_unknown_id', 'async');

// One real promise + one bogus promise id (large positive int, never returned
// by oxphp_async). await_any must reject the entire call with a clear error
// rather than silently dropping the bogus id and racing the rest.
$valid = oxphp_async(fn(): int => 42);
$bogus = 999_999_999;

$threw = false;
$message = '';
try {
    oxphp_async_await_any([$valid, $bogus]);
} catch (\Throwable $e) {
    $threw = true;
    $message = $e->getMessage();
}

$t->assertTrue('unknown-id path threw', $threw);
$t->assertContains('error mentions unknown id', $message, '999999999');
$t->assertContains('error mentions function', $message, 'oxphp_async_await_any');

// The valid promise must remain awaitable individually — the dispatcher
// restored its receiver to PROMISE_MAP after detecting the bad id.
$result = oxphp_async_await($valid);
$t->assertSame('valid promise still resolvable after bad-id rejection', $result, 42);

$t->done();
