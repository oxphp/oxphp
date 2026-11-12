<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// With session_id and session_name in disable_functions, as in this profile,
// Session::id() and name() return an empty string with a warning instead of
// throwing "Invalid callback", and they never call a function the application
// declares under either name.
$t = new TestCase('session_id_name_disabled', 'session_functions_off');

$t->assertFalse('session_id() is not available in this profile', function_exists('session_id'));
$t->assertFalse('session_name() is not available in this profile', function_exists('session_name'));

session_start();
$session = oxphp_http_request()->session();
$t->assertTrue('the session is active', $session !== null);

$warnings = [];
$collect = static function (int $errno, string $errstr) use (&$warnings): bool {
    $warnings[] = $errstr;
    return true;
};
$callCollecting = static function (string $method) use ($session, $collect): mixed {
    set_error_handler($collect, E_WARNING);
    try {
        return $session->$method();
    } catch (Throwable $e) {
        return get_class($e) . ': ' . $e->getMessage();
    } finally {
        restore_error_handler();
    }
};

foreach (['id' => 'session_id', 'name' => 'session_name'] as $method => $function) {
    $warnings = [];
    $t->assertSame("$method() returns an empty string", $callCollecting($method), '');
    $t->assertCount("$method() raises one warning", $warnings, 1);
    $t->assertMatch("$method() names $function()", $warnings[0] ?? '', "/$function\\(\\) is not available/");

    // The TestCase error handler turns that warning into an exception, as frameworks do.
    $t->assertThrows("$method() throws under a throwing error handler", fn () => $session->$method(), ErrorException::class);
}

if (true) {
    function session_id(): array
    {
        $GLOBALS['user_functions_called'][] = 'session_id';
        return [];
    }

    function session_name(): array
    {
        $GLOBALS['user_functions_called'][] = 'session_name';
        return [];
    }
}

$GLOBALS['user_functions_called'] = [];
$t->assertTrue('the application has declared its own session_id()', function_exists('session_id'));
$t->assertTrue('the application has declared its own session_name()', function_exists('session_name'));

foreach (['id' => 'session_id', 'name' => 'session_name'] as $method => $function) {
    $warnings = [];
    $t->assertSame("$method() still returns an empty string", $callCollecting($method), '');
    $t->assertCount("$method() still raises one warning", $warnings, 1);
}
$t->assertSame('neither function the application declared is called', $GLOBALS['user_functions_called'], []);

$t->done();
