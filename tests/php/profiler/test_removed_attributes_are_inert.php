<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

// Application code may still carry the profiler's former filter attributes.
// PHP resolves an attribute's class only on newInstance(), so such code has to
// keep running untouched, under a profile as well as without one.

$t = new TestCase('removed_attributes_are_inert', 'profiler');

#[OxPHP\Profile\Profile]
#[OxPHP\Profile\Exclude]
#[OxPHP\Profile\Tag('k', 'v')]
function _removed_attr_fn(): int { return 7; }

#[OxPHP\Profile\Sample(0.0)]
function _removed_attr_sampled(): int { return 8; }

#[OxPHP\Profile\Exclude]
class _RemovedAttrClass
{
    public function m(): int { return 9; }
}

OxPHP\Profile\start();
$t->assertSame('function runs', _removed_attr_fn(), 7);
$t->assertSame('sampled function runs', _removed_attr_sampled(), 8);
$t->assertSame('method runs', (new _RemovedAttrClass())->m(), 9);
OxPHP\Profile\stop();

$attr = (new ReflectionFunction('_removed_attr_fn'))->getAttributes()[0];
try {
    $attr->newInstance();
    $thrown = false;
} catch (Error $e) {
    $thrown = true;
}
$t->assertTrue('newInstance() reports the class is gone', $thrown);

$t->done();
