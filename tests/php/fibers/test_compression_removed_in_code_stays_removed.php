<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// A request's park does not re-run the handlers of the ini directives it
// changed.
//
// zlib.output_compression's handler does more than store the value: set at run
// time, it also starts zlib's output handler when none is running. PHP runs it
// once, when the script sets the directive. Run again at every resume, it would
// start the handler anew — so a request that turned compression on and then
// removed the handler in code would resume with it back on its output stack.
//
// zlib starts the handler only for a client that accepts gzip; the suite sends
// this request with --compressed.

$t = new TestCase('compression_removed_in_code_stays_removed', 'fibers');

ini_set('zlib.output_compression', '1');
$started = in_array('zlib output compression', ob_list_handlers(), true);
ob_end_clean();

// Hooked in this profile: parks this request and hands the worker back.
usleep(1000);

$handlers = ob_list_handlers();
foreach ($handlers as $_) {
    ob_end_clean();
}

$t->assertTrue('setting the directive started the handler', $started);
$t->assertSame('the handler the request removed did not come back with it', $handlers, []);

$t->done();
