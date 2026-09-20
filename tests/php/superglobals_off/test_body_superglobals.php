<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// What SUPERGLOBALS_ENABLED=false leaves standing. Everything built from the
// request body or the Cookie header survives it: the flag is read where
// $_SERVER and $_GET are built, and the body and cookie callbacks are handed
// to PHP unconditionally. This is the measurement the documented behaviour of
// the setting has to match.
$t = new TestCase('body_superglobals', 'superglobals_off');

$t->assertEqual('$_POST carries the parsed body', $_POST, ['a' => '1', 'b' => 'two']);
$t->assertEqual('$_COOKIE carries the Cookie header', $_COOKIE, ['c' => 'cookieval']);
$t->assertKeyExists('$_REQUEST has the POST half', $_REQUEST, 'a');
$t->assertKeyExists('$_REQUEST has the cookie half', $_REQUEST, 'c');
$t->assertSame('php://input is readable', file_get_contents('php://input'), 'a=1&b=two');

$t->done();
