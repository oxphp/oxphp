<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('die_in_closure', 'async');

$p = oxphp_async(function(): void {
    die('boom');
});

$t->assertThrows(
    'die() in closure throws OxPHP\\Async\\AsyncException',
    function() use ($p) { oxphp_async_await($p); },
    \OxPHP\Async\AsyncException::class
);

// Prove the pool recovered by dispatching and awaiting another task
$p2 = oxphp_async(fn() => 'pool_alive');
$t->assertSame('pool recovered after die()', oxphp_async_await($p2), 'pool_alive');

$t->done();
