<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('start_noop_when_disabled', 'profiler');

// This test runs on the `decorators` profile: the profiler is compiled in (the
// image is built with `decorator-test`, which pulls in `plugin-profiler`) and
// PROFILER_ENABLED is unset — the shape of the release image out of the box.
// The OxPHP\Profile\* functions are registered there, but the profiler's
// function observer is not, so the profiler records nothing a request does.
// start() must not raise the profiling mode then: is_active() would answer
// "being profiled" for a request that is not.

$t->assertTrue(
    'OxPHP\Profile\start is registered with the profiler disabled',
    function_exists('OxPHP\Profile\start')
);

// Premise: the profiler really is off in this profile. Otherwise start() takes
// the enabled branch and the assertions below say nothing about this one.
$t->assertSame('PROFILER_ENABLED is unset', getenv('PROFILER_ENABLED'), false);

$t->assertFalse('is_active() is false before start()', OxPHP\Profile\is_active());

OxPHP\Profile\start();
$t->assertFalse('is_active() is still false after start()', OxPHP\Profile\is_active());

// is_active() also reads the paused flag. resume() clears it, so a false here
// can only come from the mode start() left alone, not from a pause.
OxPHP\Profile\resume();
$t->assertFalse('is_active() is false with the pause cleared', OxPHP\Profile\is_active());

$t->done();
