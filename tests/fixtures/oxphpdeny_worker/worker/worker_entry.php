<?php

declare(strict_types=1);

require_once '/var/www/html/public/tests/test_helper.php';

// Every request this profile sends with X-Expect-Uri must reach the worker
// with its REQUEST_URI intact.
oxphp_worker(function (): void {
    $t = new TestCase('oxphpdeny_worker_entry', 'oxphpdeny');
    $t->assertSame(
        'REQUEST_URI',
        $_SERVER['REQUEST_URI'] ?? '',
        $_SERVER['HTTP_X_EXPECT_URI'] ?? '(no X-Expect-Uri)'
    );
    $t->done();
});
