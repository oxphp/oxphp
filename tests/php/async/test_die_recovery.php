<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('die_recovery', 'async');

for ($round = 1; $round <= 3; $round++) {
    // die() inside closure
    $pd = oxphp_async(function() use ($round): void {
        die('round' . $round);
    });

    $dieThrew = false;
    try {
        oxphp_async_await($pd);
    } catch (\OxPHP\Async\Exception $e) {
        $dieThrew = true;
    }
    $t->assertTrue("round $round: die() threw OxPHP\\Async\\Exception", $dieThrew);

    // Pool should still work
    $pr = oxphp_async(fn() => $round * 10);
    $t->assertSame("round $round: pool recovered", oxphp_async_await($pr), $round * 10);
}

$t->done();
