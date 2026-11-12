<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// Session::id() and name() answer what session_id() and session_name() do,
// including a name the script set before session_start().
$t = new TestCase('session_id_name', 'http_object');

$warnings = [];
set_error_handler(static function (int $errno, string $errstr) use (&$warnings): bool {
    $warnings[] = $errstr;
    return true;
});

session_name('OXSESS');
session_start();
$session = oxphp_http_request()->session();

$t->assertNotEmpty('the session has an ID', session_id());
$t->assertSame('id() is session_id()', $session->id(), session_id());
$t->assertSame('name() is session_name()', $session->name(), session_name());
$t->assertSame('name() is the name set before session_start()', $session->name(), 'OXSESS');
$t->assertCount('without a warning', $warnings, 0);

$t->done();
