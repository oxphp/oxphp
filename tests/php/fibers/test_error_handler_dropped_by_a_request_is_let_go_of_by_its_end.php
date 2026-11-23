<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// The server keeps its own reference to an error handler for the length of each
// call of it. A handler a request sets, has called and then drops goes by the
// end of that request, and what it holds with it: not when some later request
// on the worker happens to raise an error, nor when the worker exits.

$run = bin2hex(random_bytes(4));
$resp = OxphpErrorModeProbe::read(OxphpErrorModeProbe::send("mode=drop&run=$run", 'fixture_error_handler_park.php'));
$released = OxphpErrorModeProbe::$inWindow["released-$run"] ?? false;

$t = new TestCase('error_handler_dropped_by_a_request_is_let_go_of_by_its_end', 'fibers');

$t->assertContains('the other request had its handler heard and dropped it', $resp, "DROPPED\n");
$t->assertTrue('what the handler held was released by the time its response was sent', $released);

$t->done();
