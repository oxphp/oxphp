<?php
declare(strict_types=1);
require __DIR__ . '/../test_helper.php';

use OxPHP\Profile\Mark;

$t = new TestCase('decorator_mark', 'profiler');

#[Mark]
function decorated_marked(): int
{
    return 42;
}

OxPHP\Profile\start();

// Decorated function should run normally — the decorator attaches
// a Mark event but doesn't change the return value.
$result = decorated_marked();
$t->assertSame('decorated function returns expected value', $result, 42);

// Calling again — multiple invocations work.
$result2 = decorated_marked();
$t->assertSame('decorated function works on repeat call', $result2, 42);

OxPHP\Profile\stop();
$t->done();
