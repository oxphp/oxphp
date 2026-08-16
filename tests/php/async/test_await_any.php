<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_any', 'async');

// All three promises succeed at different speeds. The fastest fulfilled wins.
$promises = [];
$delays = [300_000, 100_000, 200_000]; // 300ms, 100ms, 200ms
foreach ($delays as $d) {
    $promises[] = oxphp_async(function (int $d): int {
        usleep($d);
        return $d;
    }, $d);
}

$winner = oxphp_async_await_any($promises);

$t->assertType('result is array', $winner, 'array');
$t->assertKeyExists('result has key id', $winner, 'id');
$t->assertKeyExists('result has key value', $winner, 'value');
// Fastest = the 100_000us promise = $promises[1].
$t->assertSame('winner is the fastest promise', $winner['id'], $promises[1]);
$t->assertSame('winner value is its delay', $winner['value'], 100_000);

$t->done();
