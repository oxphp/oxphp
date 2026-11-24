<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// With SUPERGLOBALS_ENABLED=false PHP builds no $_GET, so query() parses the
// query string itself, and the parse warns through the script's error handler.
// TestCase's handler turns a warning into an ErrorException, as framework
// handlers do. The suite passes the case in an X-Case header: $_GET is empty
// here, and query() is the call under test.
$t = new TestCase('query_parse_warnings', 'superglobals_off');

$req = oxphp_http_request();
// The nesting warning is raised only with display_errors off.
ini_set('display_errors', '0');

$thrown = static function (callable $fn): string {
    try {
        $fn();
        return 'no exception';
    } catch (\ErrorException $e) {
        return $e->getMessage();
    }
};

if ($req->header('X-Case') === 'vars') {
    $limit = (int) ini_get('max_input_vars');
    $t->assertSame('the query string carries one parameter past max_input_vars',
        substr_count((string) $req->queryString(), '='), $limit + 1);

    foreach (['query()' => fn () => $req->query(), 'query($key)' => fn () => $req->query('k1')] as $call => $fn) {
        $t->assertContains("$call throws the max_input_vars warning", $thrown($fn), "Input variables exceeded $limit");
    }

    restore_error_handler();
    error_clear_last();
    $all = $req->query();
    $t->assertContains('without a handler query() warns', error_get_last()['message'] ?? '', "Input variables exceeded $limit");
    $t->assertSame('query() holds exactly the limit', count($all), $limit);
    error_clear_last();
    $req->query('k1');
    $t->assertNull('a later call reads the kept array, without a warning', error_get_last());
} else {
    $level = (int) ini_get('max_input_nesting_level');
    $t->assertContains('query() throws the nesting warning', $thrown(fn () => $req->query()), "Input variable nesting level exceeded $level");

    // What the documentation offers code that runs under such a handler.
    error_clear_last();
    set_error_handler(static fn (): bool => false);
    try {
        $all = $req->query();
    } finally {
        restore_error_handler();
    }
    $t->assertContains('under a handler that returns false the warning is reported', error_get_last()['message'] ?? '', "Input variable nesting level exceeded $level");
    $t->assertSame('the parse drops the name it nested too deep under and goes on', $all, ['b' => '2']);
}
$t->done();
