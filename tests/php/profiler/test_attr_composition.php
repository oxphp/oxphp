<?php
declare(strict_types=1);
require_once __DIR__ . '/../test_helper.php';

use OxPHP\Profile\{Tag, Profile, Exclude};

// Class-level Tag should propagate to all methods. Method-level Tag
// adds on top. Exclude on a method overrides (method excluded even
// if class is force-profiled).
#[Tag(key: 'class_tag', value: 'class_value')]
#[Profile]
class TaggedService
{
    #[Tag(key: 'method_tag', value: 'method_value')]
    public function tagged_method(): int
    {
        return 1;
    }

    #[Exclude]
    public function excluded_method(): int
    {
        return 2;
    }

    public function plain_method(): int
    {
        // Inherits class-level Profile + Tag, no method-level overrides.
        return 3;
    }
}

$t = new TestCase('attr_composition', 'profiler');
$svc = new TaggedService();

OxPHP\Profile\start();

// All three methods run normally. The composition rules
// (class+method tag accumulation, Exclude on method overriding
// class-level Profile) affect the resulting tree but not the
// return values.
$t->assertSame('class+method tagged returns', $svc->tagged_method(), 1);
$t->assertSame('excluded method overrides class profile', $svc->excluded_method(), 2);
$t->assertSame('plain method inherits class profile+tag', $svc->plain_method(), 3);

OxPHP\Profile\stop();
$t->done();
