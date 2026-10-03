<?php

declare(strict_types=1);

require_once __DIR__ . '/tests/test_helper.php';

// The front controller. A `>` rule hands it the request even where the file
// exists on disk; the request says what REQUEST_URI should read.
$t = new TestCase('oxphpdeny_front_controller', 'oxphpdeny');
$t->assertSame('SCRIPT_NAME', $_SERVER['SCRIPT_NAME'] ?? '', '/index.php');
$t->assertSame(
    'REQUEST_URI',
    $_SERVER['REQUEST_URI'] ?? '',
    $_SERVER['HTTP_X_EXPECT_URI'] ?? '(no X-Expect-Uri)'
);
$t->done();
