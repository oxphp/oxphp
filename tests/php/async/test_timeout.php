<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('timeout', 'async');

$p = oxphp_async(function(): void {
    usleep(5000000); // 5s
});

$t->assertThrows(
    'await with 0.1s timeout throws OxPHP\\Async\\TimeoutException',
    function() use ($p) { oxphp_async_await($p, 0.1); },
    \OxPHP\Async\TimeoutException::class
);

$t->done();
