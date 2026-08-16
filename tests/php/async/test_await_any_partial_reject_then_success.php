<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_any_partial_reject_then_success', 'async');

// p1 rejects fast (50ms), p2 succeeds slower (150ms).
// await_any must wait past p1's rejection and return p2's success.
$p1 = oxphp_async(function (): never {
    usleep(50_000);
    throw new \RuntimeException('first one fails');
});
$p2 = oxphp_async(function (): string {
    usleep(150_000);
    return 'late winner';
});

$winner = oxphp_async_await_any([$p1, $p2], 5.0);

$t->assertType('result is array', $winner, 'array');
$t->assertKeyExists('result has key id', $winner, 'id');
$t->assertKeyExists('result has key value', $winner, 'value');
$t->assertSame('winner is the slow success', $winner['id'], $p2);
$t->assertSame('winner value matches', $winner['value'], 'late winner');

$t->done();
