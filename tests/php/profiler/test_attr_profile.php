<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

use OxPHP\Profile\Profile;

#[Profile]
function force_profiled(): int
{
    return 7;
}

$t = new TestCase('attr_profile', 'profiler');

// Without start() and without a trigger header, profiling is OFF.
// #[Profile] forces this fn's spans to be created anyway. We can't
// directly inspect the tree from PHP yet, so the assertion is
// functional: the decorated fn runs cleanly.
$result = force_profiled();
$t->assertSame('force-profiled fn returns expected value', $result, 7);

// And with profiling on, the same fn still runs.
OxPHP\Profile\start();
$result2 = force_profiled();
$t->assertSame('force-profiled fn works under start()', $result2, 7);
OxPHP\Profile\stop();

$t->done();
