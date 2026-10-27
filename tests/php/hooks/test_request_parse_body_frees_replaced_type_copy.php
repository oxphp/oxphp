<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Every parse keeps a copy of the whole Content-Type header, as long as the
// client cares to send it, and the next parse writes its own copy over it
// without freeing it. A worker's heap outlives the request, so a copy lost that
// way stays lost. The suite lines send a header over 100 KB so that the copy
// stands out of everything else the call allocates.
//
// case=nested: the error handler of a parse makes a request_parse_body() call
// that succeeds, replacing the copy the outer call had set.
// case=unparsed: a type PHP has no parser for, whose copy nothing consumes,
// replaced by the call's own.

$t = new TestCase('request_parse_body_frees_replaced_type_copy', 'hooks');

$case = (string) ($_GET['case'] ?? '');
$t->assertGreaterThan(
    'the content type is long enough to measure',
    strlen((string) ($_SERVER['CONTENT_TYPE'] ?? '')),
    100000
);

if ($case === 'nested') {
    $entered = false;
    $inner = null;
    set_error_handler(static function () use (&$entered, &$inner): bool {
        if (!$entered) {
            $entered = true;
            // A limit of its own: the inner call would otherwise run under the
            // outer one's, and warn while this handler is running, which PHP
            // sends straight to the output.
            $inner = request_parse_body(['post_max_size' => 0]);
        }
        return true;
    });
    $before = memory_get_usage();
    try {
        request_parse_body(['post_max_size' => 1]);
    } finally {
        restore_error_handler();
    }
    $grown = memory_get_usage() - $before;
    // The premise: the inner call ran inside the outer one and went through a
    // whole parse, which is what replaces the copy.
    $t->assertType('the inner call ran a whole parse', $inner, 'array');
} else {
    $before = memory_get_usage();
    $refused = false;
    try {
        request_parse_body();
    } catch (\RequestParseBodyException) {
        $refused = true;
    }
    $grown = memory_get_usage() - $before;
    $t->assertTrue('the call refuses the type', $refused);
}

$t->meta('memory_grown_by_the_call', $grown);
$t->assertLessThan('the replaced copy is freed', $grown, 50000);

$t->done();
