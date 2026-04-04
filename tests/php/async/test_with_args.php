<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('with_args', 'async');

$p = oxphp_async(fn($a, $b) => $a + $b, 10, 20);
$t->assertSame('dispatch with args returns 30', oxphp_async_await($p), 30);

$t->done();
