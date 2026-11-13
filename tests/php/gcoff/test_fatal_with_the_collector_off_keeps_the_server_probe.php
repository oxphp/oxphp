<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/gcoff_state.php';

// Follows gcoff/test_fatal_with_the_collector_off_keeps_the_server, on the
// worker it ran on.

$t = new TestCase('fatal_with_the_collector_off_keeps_the_server_probe', 'gcoff');

$t->assertSame(
    'this request runs on the worker the fatal ended a request on',
    OxPHP\Server\Worker::current()->startTime(),
    oxphp_gcoff_recorded_worker()
);

$status = gc_status();
$t->assertFalse('the collector is off, as php.ini says', gc_enabled());
$t->assertSame('so it has no buffer of possible roots', $status['buffer_size'], 0);
$t->assertSame('its guard was up before the fatal', oxphp_gcoff_recorded('protected'), true);
$t->assertTrue('and is up after it', $status['protected']);

$t->done();
