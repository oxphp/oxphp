<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('superglobals_reset', 'worker');

// $_GET should only contain keys sent in this specific request,
// not leftovers from previous requests handled by the same worker.
$t->assertType('$_GET is array', $_GET, 'array');

// $_SERVER must be populated for this request
$t->assertKeyExists('$_SERVER has REQUEST_METHOD', $_SERVER, 'REQUEST_METHOD');
$t->assertKeyExists('$_SERVER has REQUEST_URI', $_SERVER, 'REQUEST_URI');

$t->done();
