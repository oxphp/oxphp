<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('bad_args_resource', 'async');

$r = fopen('php://memory', 'r');
$t->assertThrows(
    'resource arg throws OxPHP\\Async\\AsyncException',
    function() use ($r) {
        oxphp_async(fn($x) => $x, $r);
    },
    \OxPHP\Async\AsyncException::class
);
if (is_resource($r)) {
    fclose($r);
}

$t->done();
