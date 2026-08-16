<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('bad_args_object', 'async');

$t->assertThrows(
    'object arg throws OxPHP\\Async\\AsyncException',
    function() {
        oxphp_async(fn($x) => $x, new \stdClass());
    },
    \OxPHP\Async\AsyncException::class
);

$t->done();
