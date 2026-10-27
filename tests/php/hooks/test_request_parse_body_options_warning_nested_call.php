<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// request_parse_body() called again from the error handler while the outer
// call is still reading its options: a quantity it cannot parse warns before
// anything is read. Both calls find the same Content-Type copy standing, left
// by the parse before the script (nothing consumes it for a type PHP has no
// parser for), and only one of them may free it.
//
// A copy freed twice goes back to the allocator twice. The string the handler
// allocates after the inner call lands in that memory, the second free hands
// it out again, and the next string of its size class overwrites it. The
// header is sized so that its copy (length + NUL) and each of the strings
// below share one size class, 97 to 112 bytes. The lengths come from the query
// so that no string here is built at compile time.

$t = new TestCase('request_parse_body_options_warning_nested_call', 'hooks');

$n = (int) ($_GET['n'] ?? 0);
$copy = strlen((string) ($_SERVER['CONTENT_TYPE'] ?? '')) + 1;
$t->assertTrue('the copy sits in the size class of the strings', $copy > 96 && $copy <= 112);
$t->assertSame('the query names the string length', $n, 80);

$inner = null;
$held = null;
set_error_handler(static function () use (&$inner, &$held, $n): bool {
    if ($inner === null) {
        try {
            request_parse_body();
            $inner = 'returned';
        } catch (\RequestParseBodyException) {
            $inner = 'refused';
        }
        $held = str_repeat('h', $n);
    }
    return true;
});
try {
    request_parse_body(['max_input_vars' => '1000Q']);
    $outer = 'returned';
} catch (\RequestParseBodyException) {
    $outer = 'refused';
} finally {
    restore_error_handler();
}
$probe = str_repeat('p', $n);

// The premise: the inner call ran from the options warning, before the outer
// call read anything.
$t->assertSame('the inner call ran from the warning and refused the type', $inner, 'refused');
$t->assertSame('the outer call refused the type', $outer, 'refused');
$t->assertSame('the string allocated in between is still its own', $held, str_repeat('h', 80));
$t->assertSame('and so is the one allocated after', $probe, str_repeat('p', 80));

$t->done();
