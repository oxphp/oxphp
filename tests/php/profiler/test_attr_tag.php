<?php
declare(strict_types=1);
require __DIR__ . '/../test_helper.php';

use OxPHP\Profile\Tag;

#[Tag(key: 'env', value: 'test')]
#[Tag(key: 'service', value: 'oxphp')]
function tagged_fn(): int
{
    return 99;
}

#[Tag(key: 'one', value: '1')]
function single_tag_fn(): int
{
    return 100;
}

$t = new TestCase('attr_tag', 'profiler');

OxPHP\Profile\start();

// Repeated #[Tag] accumulates; functional invariant — the fn runs.
$t->assertSame('multi-tagged fn returns', tagged_fn(), 99);

// Single-tag variant.
$t->assertSame('single-tagged fn returns', single_tag_fn(), 100);

// Calling repeatedly is fine (the spec is cached after first
// observation).
for ($i = 0; $i < 3; $i++) {
    $t->assertSame('tagged fn idempotent', tagged_fn(), 99);
}

OxPHP\Profile\stop();
$t->done();
