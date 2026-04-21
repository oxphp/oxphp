<?php
declare(strict_types=1);
require __DIR__ . '/../test_helper.php';

use OxPHP\Profile\Exclude;

#[Exclude]
function excluded_fn(): int
{
    return 42;
}

#[Exclude]
function excluded_caller(): int
{
    return excluded_fn() + 1;
}

$t = new TestCase('attr_exclude', 'profiler');

OxPHP\Profile\start();

// #[Exclude] prevents span creation but the function still runs
// normally — the decorator only affects observability.
$t->assertSame('excluded fn returns', excluded_fn(), 42);

// Nested excluded calls also run cleanly.
$t->assertSame('excluded caller returns', excluded_caller(), 43);

OxPHP\Profile\stop();
$t->done();
