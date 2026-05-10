<?php

declare(strict_types=1);

require __DIR__ . '/../test_helper.php';

$t = new TestCase('await_any_empty_array', 'async');

// Empty input array must throw — racing/picking from nothing is a programmer error.
$threw = false;
$message = '';
try {
    oxphp_async_await_any([]);
} catch (\Throwable $e) {
    $threw = true;
    $message = $e->getMessage();
}
$t->assertTrue('empty array path threw', $threw);
$t->assertContains('error message mentions empty', $message, 'must not be empty');

$t->done();
