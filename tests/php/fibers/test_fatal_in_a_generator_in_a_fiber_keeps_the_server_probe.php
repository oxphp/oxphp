<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Follows fibers/test_fatal_in_a_generator_in_a_fiber_keeps_the_server, on the
// worker that replaced the one it ran on. The replacement is the point rather
// than a detail: the worker it replaced tore down its object store on the way
// out, and that teardown is what closes the generator the previous line
// abandoned inside a fiber. Reaching here means the close did not take the
// process with it.

$t = new TestCase('fatal_in_a_generator_in_a_fiber_keeps_the_server_probe', 'fibers');

$file = '/tmp/oxphp-generator-fiber-fatal-state';
$state = is_file($file) ? json_decode((string) file_get_contents($file), true) : null;
$t->assertTrue('the previous line left its state', is_array($state));
$t->meta('state', $state);

// The shape under test. Without all three the rest of this passes for a reason
// that has nothing to do with the fix: the generator has to have been running,
// and to have had a call of its own still pending — which is what reaching the
// call nested in the yield, and never completing the yield, say between them.
$t->assertTrue('the generator ran', ($state['generator_started'] ?? false) === true);
$t->assertTrue('and reached the call nested in its yield', ($state['inner_call_entered'] ?? false) === true);
$t->assertTrue('and never got to yield a value', ($state['yielded'] ?? true) === false);

$t->assertContains('the fiber ran out of memory', (string) ($state['last_error'] ?? ''), 'Allowed memory size');

// Both halves of the trigger: the worker was asked to go, and the worker serving
// this request is a later one. Without the second, the teardown that closes the
// abandoned generator has not happened yet and nothing here has been tested.
$t->assertTrue('the previous line asked its worker to go', ($state['exit_scheduled'] ?? false) === true);
$t->assertTrue(
    'and a worker spawned after it is serving this request',
    OxPHP\Server\Worker::current()->startTime() > (float) ($state['worker_start_time'] ?? INF)
);

// Given back by the worker, from the frame that started the fiber down.
$t->assertTrue('what the request held below the fiber did not outlive it', ($state['freed'] ?? false) === true);

$t->done();
