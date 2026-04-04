<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('exception_propagation', 'async');

$p = oxphp_async(function(): void {
    throw new \RuntimeException('test');
});

$t->assertThrows(
    'await throws OxPHP\\Async\\Exception',
    function() use ($p) { oxphp_async_await($p); },
    \OxPHP\Async\Exception::class
);

$t->done();
