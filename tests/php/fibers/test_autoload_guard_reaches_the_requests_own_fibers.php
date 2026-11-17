<?php

declare(strict_types=1);

// The engine's guard against autoloading a class from inside its own autoloader
// holds in every fiber of the request that is loading it. The engine keeps the
// class names its autoloaders are loading in a table, and a lookup that finds
// the name there answers "not found" without calling them. A lookup of the same
// class from another fiber of the request has to find it there too: from a
// Fiber the autoloader starts, and from the fiber the engine runs the garbage
// collector's destructors in whenever a fiber is running, which in a request
// it always is. Otherwise the autoloader runs again for a class it is still
// loading — and declares it twice, or never stops.
//
// The autoloader itself makes the lookup, once from a Fiber it starts, once from
// the destructor of a cycle it hands to the collector.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/autoload_guard_probe.php';

$t = new TestCase('autoload_guard_reaches_the_requests_own_fibers', 'fibers');
$id = bin2hex(random_bytes(4));

foreach (['fiber', 'gc'] as $case) {
    $class = 'OxphpAutoloadGuardNested' . ucfirst($case) . $id;
    $key = strtolower($class);
    OxphpAutoloadGuard::arm($class, $case);

    $exists = class_exists($class);

    $t->assertTrue(
        "[$case] the lookup ran in a fiber other than the autoloader's",
        OxphpAutoloadGuard::$flags["$key:other_fiber"] ?? null
    );
    $t->assertSame(
        "[$case] it did not reach the autoloader again",
        OxphpAutoloadGuard::$flags["$key:reentered"] ?? null,
        false
    );
    $t->assertSame("[$case] and did not find the class", OxphpAutoloadGuard::$flags["$key:found_inside"] ?? null, false);
    $t->assertTrue("[$case] which the autoloader then declared", $exists);
}

$t->done();
