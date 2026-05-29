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

// With an explicit label the Mark event is named after it; a bare
// #[Mark] falls back to the function name. Both run normally — the
// emitted event names are asserted in the Rust harness.
#[Mark(label: "checkout")]
function decorated_labelled(): int
{
    return 7;
}

OxPHP\Profile\start();

// Decorated function should run normally — the decorator attaches
// a Mark event but doesn't change the return value.
$result = decorated_marked();
$t->assertSame('decorated function returns expected value', $result, 42);

// Calling again — multiple invocations work.
$result2 = decorated_marked();
$t->assertSame('decorated function works on repeat call', $result2, 42);

// Labelled #[Mark(label: ...)] runs without altering the return value.
$labelled = decorated_labelled();
$t->assertSame('labelled mark function returns expected value', $labelled, 7);

OxPHP\Profile\stop();
$t->done();
