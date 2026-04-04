<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('bad_args_object', 'async');

$t->assertThrows(
    'object arg throws OxPHP\\Async\\Exception',
    function() {
        oxphp_async(fn($x) => $x, new \stdClass());
    },
    \OxPHP\Async\Exception::class
);

$t->done();
