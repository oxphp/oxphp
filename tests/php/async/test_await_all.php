<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('await_all', 'async');

$promises = [];
for ($i = 0; $i < 5; $i++) {
    $promises[] = oxphp_async(function(int $n): int {
        return $n * $n;
    }, $i);
}

$results = oxphp_async_await_all($promises);
$values = array_values($results);
sort($values);

$t->assertSame('await_all returns [0,1,4,9,16]', $values, [0, 1, 4, 9, 16]);

$t->done();
