<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('strings_frozen', 'async');

$s = 'hello';
$p = oxphp_async(function() use ($s): string {
    return $s;
});

$t->assertSame('use() string is frozen and readable on worker', oxphp_async_await($p), 'hello');

$t->done();
