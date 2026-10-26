<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// request_parse_body() called again from the error handler a parse runs, by a
// call that fails before it reads anything. The outer parse has read the body
// and set the content-type copy its post handler is handed afterwards; the
// inner call must leave that copy alone, so the outer call returns what PHP
// returns for it — the body standing, parsed. php-cgi 8.4 and 8.5 give
// [['probe' => 'nested'], []] for this exact sequence.

$t = new TestCase('request_parse_body_nested_call_keeps_outer_parse', 'hooks');

$nested = null;
set_error_handler(static function () use (&$nested): bool {
    try {
        request_parse_body(['bogus' => 1]);
        $nested = 'returned';
    } catch (\ValueError $e) {
        $nested = 'ValueError: ' . $e->getMessage();
    }
    return true;
});
try {
    $parsed = request_parse_body(['post_max_size' => 1]);
} finally {
    restore_error_handler();
}

// The premise: the handler ran inside the outer call, and the inner call failed
// on its options, before reading the body.
$t->assertSame(
    'the inner call failed before reading anything',
    $nested,
    'ValueError: Invalid key "bogus" in $options argument'
);
$t->assertSame('the outer call parsed the body as PHP does', $parsed, [['probe' => 'nested'], []]);

$t->done();
