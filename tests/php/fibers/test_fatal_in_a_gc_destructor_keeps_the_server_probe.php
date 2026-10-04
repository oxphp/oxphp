<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Follows fibers/test_fatal_in_a_gc_destructor_keeps_the_server, on the worker
// that replaced the one it ran on — which takes a server still running to happen
// at all. What the previous line saw is read from the file it left.

$t = new TestCase('fatal_in_a_gc_destructor_keeps_the_server_probe', 'fibers');

$file = '/tmp/oxphp-gc-destructor-fatal-state';
$state = is_file($file) ? json_decode((string) file_get_contents($file), true) : null;
$t->assertTrue('the previous line left its state', is_array($state));
$t->meta('state', $state);

// The route under test: the destructor ran in a fiber, and not in the request's.
// A collector that called it on the request's own stack would make the rest of
// this pass for a reason that has nothing to do with the fix.
$t->assertNotNull('the request ran in a fiber', $state['request_fiber'] ?? null);
$t->assertNotNull('the destructor ran in a fiber', $state['destructor_fiber'] ?? null);
$t->assertTrue(
    'a fiber other than the request\'s',
    ($state['destructor_fiber'] ?? null) !== ($state['request_fiber'] ?? null)
);

$t->assertContains('the destructor ran out of memory', (string) ($state['last_error'] ?? ''), 'Allowed memory size');

// Given back by the worker, from the frame the collection ran from down.
$t->assertTrue('what the request held below the collection did not outlive it', ($state['freed'] ?? false) === true);

$t->done();
