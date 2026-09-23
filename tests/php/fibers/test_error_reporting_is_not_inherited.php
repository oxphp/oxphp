<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';

// The reporting level a request sets with error_reporting() ends with that
// request.
//
// A worker keeps its request fibers and hands each one the next request once
// the last has finished. The engine carries the live reporting level with the
// fiber — it is saved when a fiber is switched away from and put back when it
// is switched to — so anything that leaves the level where the last request put
// it hands it to the next request on the same fiber. A request that turned
// reporting down around a noisy library would silence every later request on
// that fiber, and one that turned it up would fill their logs.
//
// Two inner requests, one after the other, while this one is parked on each:
// the first lowers the level and finishes, the second reports what it starts
// with. They have to land on the same fiber for this to ask anything, so that
// is checked rather than assumed.

$t = new TestCase('error_reporting_is_not_inherited', 'fibers');

$set = json_decode(fiber_inner_request('/tests/fibers/fixture_error_reporting_probe.php?phase=set'), true);
$read = json_decode(fiber_inner_request('/tests/fibers/fixture_error_reporting_probe.php?phase=read'), true);

$t->assertTrue('both inner requests answered with JSON', is_array($set) && is_array($read));
if (!is_array($set) || !is_array($read)) {
    $t->done();
}

$t->assertSame('the first request lowered its own level', $set['level'], $set['sentinel']);

$t->assertTrue(
    'both requests were served by the same fiber (' . var_export($set['fiber'], true)
        . ' / ' . var_export($read['fiber'], true) . ') — otherwise nothing was carried and this is'
        . ' not a test',
    $set['fiber'] !== null && $set['fiber'] === $read['fiber']
);

$t->assertNotEqual(
    'the next request on that fiber does not start at the level the last one set',
    $read['level'],
    $read['sentinel']
);
// A directive nothing set reads as '' and means E_ALL — the rule the engine
// applies when it seeds a new fiber's level from it.
$directive = ($read['ini'] === '' || $read['ini'] === false) ? E_ALL : (int) $read['ini'];
$t->assertSame(
    'and starts where the directive says it does',
    $read['level'],
    $directive
);

$t->done();
