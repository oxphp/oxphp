<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// The server keeps a reference of its own to an error handler for each call of
// it. A handler a request sets, has called and then drops is not released inside
// PHP's reporting of the request's next error: what the release runs — here a
// destructor that throws — would stand in for that error, and the handler then
// installed would never hear it. It is still released, by the request's end.

$run = bin2hex(random_bytes(4));
$resp = OxphpErrorModeProbe::read(OxphpErrorModeProbe::send("mode=next&run=$run", 'fixture_error_handler_park.php'));
$released = OxphpErrorModeProbe::$inWindow["released-$run"] ?? false;

$t = new TestCase('error_handler_dropped_by_a_request_is_not_released_inside_a_later_error', 'fibers');

$t->assertContains('premise: the dropped handler was still held when the next error was raised', $resp, "HELD:yes\n");
$t->assertContains('the error raised after the drop: nothing thrown in its place', $resp, "THROWN:none\n");
$t->assertContains('and the handler installed then heard it', $resp, "HEARD:next-$run\n");
$t->assertTrue('what the dropped handler held was released by the time the response was sent', $released);

$t->done();
