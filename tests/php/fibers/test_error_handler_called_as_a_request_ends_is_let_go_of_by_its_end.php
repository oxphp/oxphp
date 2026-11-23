<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/error_mode_probe.php';

// The server keeps its own reference to an error handler for each call of it,
// and lets go of those a request made by the end of that request — including a
// call made once the request's own code is over, from a destructor that letting
// go of a fatal error's backtrace ran, and a handler that destructor then
// dropped: not when the fiber the request ran in is handed another one.

$run = bin2hex(random_bytes(4));
$resp = OxphpErrorModeProbe::read(OxphpErrorModeProbe::send("mode=late&run=$run", 'fixture_error_handler_park.php'));
$heardAfterReturn = OxphpErrorModeProbe::$inWindow["late-after-return-$run"] ?? null;
$released = OxphpErrorModeProbe::$inWindow["released-$run"] ?? false;

$t = new TestCase('error_handler_called_as_a_request_ends_is_let_go_of_by_its_end', 'fibers');

$t->assertContains('the other request set its handler and returned', $resp, "ARMED\n");
$t->assertSame(
    PHP_VERSION_ID >= 80500
        ? "premise: its handler heard the destructor once the request's code was over, as its end let go of the backtrace"
        : 'with no backtrace, its handler heard the destructor as the call returned',
    $heardAfterReturn,
    PHP_VERSION_ID >= 80500
);
$t->assertTrue('what the handler held was released by the time its response was sent', $released);

$t->done();
