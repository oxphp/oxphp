<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('bad_args_resource', 'async');

$r = fopen('php://memory', 'r');
$t->assertThrows(
    'resource arg throws OxPHP\\Async\\Exception',
    function() use ($r) {
        oxphp_async(fn($x) => $x, $r);
    },
    \OxPHP\Async\Exception::class
);
if (is_resource($r)) {
    fclose($r);
}

$t->done();
