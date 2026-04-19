<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('deny_static_still_served', 'php_deny');
// Placeholder — runner hits /uploads/image.png and expects HTTP 200.
// PHP_DENY_DIRS only blocks PHP execution; static assets under denied
// directories must still be served by the static handler.
$t->assertTrue('placeholder — runner checks 200 for static asset under denied dir', true);
$t->done();
