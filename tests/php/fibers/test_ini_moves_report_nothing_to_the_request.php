<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// Taking a request's ini changes off the worker at a park, and applying them
// again on resume, reports nothing to that request.
//
// Applying a value runs the directive's handler, and even a handler that only
// stores a value can diagnose one: a quantity it cannot fully parse is taken
// with a warning. The request heard that once, when it made the change. It
// must not hear it again at every resume — and it would, in its own response,
// if the moves ran at the request's reporting level, because error_reporting is
// one of the directives being moved and its handler raises the level as it
// goes.
//
// So the order matters: display_errors, then error_reporting, then the
// directive that diagnoses. Applied in that order on resume, the diagnosis
// lands after the level has been restored to one that shows it.

$t = new TestCase('ini_moves_report_nothing_to_the_request', 'fibers');

ini_set('display_errors', '1');
// A level other than the one in force, or error_reporting() changes nothing and
// the directive is not among the ones moved; both include warnings.
error_reporting(error_reporting() === E_ALL ? E_ALL & ~E_NOTICE : E_ALL);
// The one time the request is told: taken here, so the helper's handler does not
// turn it into an exception.
set_error_handler(static fn (): bool => true, E_WARNING);
ini_set('default_socket_timeout', '7 apples');
restore_error_handler();

$t->assertSame('default_socket_timeout took before the park', ini_get('default_socket_timeout'), '7 apples');

ob_start();
// Hooked in this profile: parks this request and hands the worker back.
usleep(1000);
$shown = ob_get_clean();

$t->assertSame(
    'the request resumed with its own default_socket_timeout',
    ini_get('default_socket_timeout'),
    '7 apples'
);
$t->assertTrue(
    'and nothing was reported into its output on the way back (' . json_encode($shown) . ')',
    !str_contains($shown, 'default_socket_timeout')
);

$t->done();
