<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('parallel_wall_time', 'async');

$start = hrtime(true);

$promises = [];
for ($i = 0; $i < 4; $i++) {
    $promises[] = oxphp_async(function(): void {
        usleep(200000); // 200ms each
    });
}

oxphp_async_await_all($promises);

$elapsed_ms = (int)((hrtime(true) - $start) / 1_000_000);

$t->assertTrue('4 x 200ms tasks complete in < 600ms (parallel)', $elapsed_ms < 600);

$t->meta('elapsed_ms', $elapsed_ms);

$t->done();
