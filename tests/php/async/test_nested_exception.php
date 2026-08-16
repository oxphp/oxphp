<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('nested_exception', 'async');

$p = oxphp_async(function(): never {
    throw new \DomainException('inner');
});

$caught = false;
$msg = '';
try {
    oxphp_async_await($p);
} catch (\OxPHP\Async\AsyncException $e) {
    $caught = true;
    $msg = $e->getMessage();
}

$t->assertTrue('OxPHP\\Async\\AsyncException was thrown', $caught);
$t->assertTrue(
    'message contains DomainException or inner',
    str_contains($msg, 'DomainException') || str_contains($msg, 'inner')
);

$t->done();
