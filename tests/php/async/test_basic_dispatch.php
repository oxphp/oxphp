<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('basic_dispatch', 'async');

$p = oxphp_async(fn() => 42);
$r = oxphp_async_await($p);
$t->assertSame('dispatch + await returns 42', $r, 42);

$t->done();
