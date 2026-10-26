<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Follows fibers/test_fatal_in_a_gc_destructor_keeps_the_request_fiber_current,
// on the worker that replaced the one it ran on. What the previous line's
// shutdown function saw is read from the file it left.

$t = new TestCase('fatal_in_a_gc_destructor_keeps_the_request_fiber_current_probe', 'fibers');

$file = '/tmp/oxphp-gc-destructor-fiber-current-state';
$state = is_file($file) ? json_decode((string) file_get_contents($file), true) : null;
$t->assertTrue('the previous line left its state', is_array($state));
$t->meta('state', $state);

// The route under test: the destructor ran in the collector's fiber, and that is
// where the fatal came from.
$t->assertNotNull('the request ran in a fiber', $state['request_fiber'] ?? null);
$t->assertNotNull('the destructor ran in a fiber', $state['destructor_fiber'] ?? null);
$t->assertTrue(
    'a fiber other than the request\'s',
    ($state['destructor_fiber'] ?? null) !== ($state['request_fiber'] ?? null)
);
$t->assertContains('and fataled there', (string) ($state['last_error'] ?? ''), 'OxphpGcDestructorFiberCurrent');

$t->assertSame(
    'the request\'s shutdown function ran in the request\'s fiber',
    $state['shutdown_fiber'] ?? null,
    $state['request_fiber'] ?? null
);

// A collection asked for after the fatal returns at once: the interrupted run
// is still marked as running, and the collector's fiber the fatal destroyed is
// never resumed. That this file was written at all, and that this line is served,
// says the process came through it.
$t->assertSame('a collection after the fatal collected nothing', $state['collected'] ?? null, 0);
$t->assertTrue('because the interrupted one is still marked as running', ($state['gc_running'] ?? false) === true);

$t->assertTrue('the previous line\'s worker was asked to go', ($state['exit_scheduled'] ?? false) === true);
$t->assertTrue(
    'and a worker spawned after it is serving this request',
    OxPHP\Server\Worker::current()->startTime() > (float) ($state['worker_start_time'] ?? INF)
);

$t->done();
