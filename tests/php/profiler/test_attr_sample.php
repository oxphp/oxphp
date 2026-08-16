<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

use OxPHP\Profile\Sample;

#[Sample(rate: 0.0)]
function never_sampled(): int
{
    return 1;
}

#[Sample(rate: 1.0)]
function always_sampled(): int
{
    return 2;
}

#[Sample(rate: 0.5)]
function maybe_sampled(): int
{
    return 3;
}

$t = new TestCase('attr_sample', 'profiler');

OxPHP\Profile\start();

// rate=0.0 → bridge always returns SKIP from sample_hit; the fn
// itself still runs (the attribute only gates span creation).
for ($i = 0; $i < 5; $i++) {
    $t->assertSame('never_sampled returns under rate=0.0', never_sampled(), 1);
}

// rate=1.0 → always captured; fn returns normally.
for ($i = 0; $i < 5; $i++) {
    $t->assertSame('always_sampled returns under rate=1.0', always_sampled(), 2);
}

// rate=0.5 → fn runs cleanly regardless of the dice roll.
for ($i = 0; $i < 10; $i++) {
    $t->assertSame('maybe_sampled returns under rate=0.5', maybe_sampled(), 3);
}

OxPHP\Profile\stop();
$t->done();
