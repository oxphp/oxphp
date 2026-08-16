<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('not_awaited_cleanup', 'async');

// Dispatch a promise but do NOT await it — RSHUTDOWN cleans up
$p = oxphp_async(function(): string {
    usleep(50000);
    return 'orphan';
});

// Just assert dispatch succeeded (returned an int promise ID)
$t->assertType('dispatch returns integer promise id', $p, 'integer');

// Do not call oxphp_async_await($p) — RSHUTDOWN handles cleanup

$t->done();
