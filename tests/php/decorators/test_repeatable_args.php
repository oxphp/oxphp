<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

use OxPHP\Test\Mark;

$t = new TestCase('repeatable_args', 'decorators');

// `OxPHP\Test\Mark` is a test-only decorator (server built with the
// `decorator-test` feature): repeatable and ALL-target. Each invocation of a
// decorated function records the decorator's resolved per-occurrence label;
// `OxPHP\Test\decorator_labels()` drains and returns them comma-joined. This
// makes per-(name, scope) attribute-occurrence resolution observable from PHP.

// Clear any residue left on this worker by a prior run of this script.
OxPHP\Test\decorator_labels();

// ── Case 1: repeatable attribute on a function ──
// Each occurrence must read its OWN constructor argument — "a" then "b".
// The pre-fix resolver read occurrence 0 twice and produced "a,a".
#[Mark("a")]
#[Mark("b")]
function decorated_repeatable(): int
{
    return 1;
}

$r1 = decorated_repeatable();
$t->assertSame('decorated function returns expected value', $r1, 1);
$labels1 = OxPHP\Test\decorator_labels();
$t->assertSame('repeatable attribute reads each occurrence', $labels1, 'a,b');

// ── Case 2: same attribute on the class AND the method ──
// Function scope resolves first, then class scope, and the two scopes must
// not alias occurrence 0 — labels are "mth" (method) then "cls" (class).
// The pre-fix resolver read fn-scope occurrence 0 twice and produced "mth,mth".
#[Mark("cls")]
class DecoratedClass
{
    #[Mark("mth")]
    public function run(): int
    {
        return 2;
    }
}

$r2 = (new DecoratedClass())->run();
$t->assertSame('decorated method returns expected value', $r2, 2);
$labels2 = OxPHP\Test\decorator_labels();
$t->assertSame('function and class scope resolve independently', $labels2, 'mth,cls');

$t->done();
