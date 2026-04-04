<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('arrays_frozen', 'async');

$arr = [1, 2, 3];
$p = oxphp_async(function() use ($arr): array {
    return $arr;
});

$t->assertSame('use() array is frozen and readable on worker', oxphp_async_await($p), [1, 2, 3]);

$t->done();
