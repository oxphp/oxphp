<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/ini_put_back_probe.php';

// Follows fibers/test_ini_put_back_fatal_retires_the_worker, on the worker that
// replaced the one it ran on — whose statics are gone, so what it left is read
// from a file.

$t = new TestCase('ini_put_back_fatal_retires_the_worker_probe', 'fibers');

$state = is_file(OxphpIniPutBackProbe::FATAL_STATE)
    ? json_decode((string) file_get_contents(OxphpIniPutBackProbe::FATAL_STATE), true)
    : null;

$t->assertTrue('the previous line left its state', is_array($state));
$t->assertTrue('the destructor the put-back ran had its fatal', ($state['ran'] ?? false) === true);
$t->assertNotNull('the scheduled recycles were read before', $state['scheduled'] ?? null);

// The recycle is counted as the worker's thread is reaped, which can come a
// moment after this request is taken by its replacement.
$after = null;
$deadline = microtime(true) + 5.0;
do {
    $after = oxphp_ini_put_back_scheduled_recycles();
    if ($after !== null && $after > ($state['scheduled'] ?? PHP_INT_MAX)) {
        break;
    }
    usleep(50_000);
} while (microtime(true) < $deadline);

$t->assertSame('the worker it ran on retired', $after, ($state['scheduled'] ?? 0) + 1);

$t->done();
