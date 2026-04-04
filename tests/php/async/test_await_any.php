<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('await_any', 'async');

$promises = [];
$delays = [300000, 100000, 200000]; // 300ms, 100ms, 200ms
foreach ($delays as $d) {
    $promises[] = oxphp_async(function(int $delay): int {
        usleep($delay);
        return $delay;
    }, $d);
}

$result = oxphp_async_await_any($promises);

$t->assertType('result is array', $result, 'array');
$t->assertKeyExists('result has key: id', $result, 'id');
$t->assertKeyExists('result has key: value', $result, 'value');

$t->done();
