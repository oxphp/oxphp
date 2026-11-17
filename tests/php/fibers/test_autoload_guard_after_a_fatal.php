<?php

declare(strict_types=1);

// A fatal inside an autoloader does not leave the class marked as being loaded.
// The engine adds the name to a table before it calls the autoloaders and takes
// it out after they return; a lookup that finds the name there answers "not
// found" without calling them, which is what stops an autoloader from recursing
// into itself. A fatal skips the taking out, so left alone the name stays: every
// later lookup of that class on the worker fails without calling the autoloader,
// for the rest of the worker's life.
//
// Checked in the failing request's own shutdown function, which runs on the
// stack the fatal left, and in the next request on the worker once the failing
// one has ended — for another class, which no shutdown function has looked up
// and declared by then. Each once with the autoloader running in the handler,
// once inside a Fiber the handler started. The next request also keeps the
// failures from running three in a row, which would retire the worker and this
// request with it.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/fiber_park_registry.php';
require_once __DIR__ . '/autoload_guard_probe.php';

$t = new TestCase('autoload_guard_after_a_fatal', 'fibers');
$id = bin2hex(random_bytes(4));

foreach (['request', 'fiber'] as $case) {
    foreach (['shutdown', 'next'] as $check) {
        $class = 'OxphpAutoloadGuardFatal' . ucfirst($case) . ucfirst($check) . $id;
        OxphpAutoloadGuard::arm($class, 'fatal');
        $sharedState['autoload_guard_shutdown'] = null;

        $failed = fiber_inner_request(
            "/tests/fibers/fixture_autoload_guard.php?role=fail&case=$case&check=$check&cls=$class",
            8.0
        );
        $t->assertNotContains("[$case/$check] the failing request failed in its autoloader", $failed, 'not reached');

        if ($check === 'shutdown') {
            $shutdown = $sharedState['autoload_guard_shutdown'];
            $t->meta("{$case}_shutdown", $shutdown);
            $t->assertTrue("[$case] it ran its shutdown functions", is_array($shutdown));
            $t->assertSame(
                "[$case] a lookup in its shutdown function reaches the autoloader again",
                $shutdown['calls'] ?? null,
                2
            );
            $t->assertTrue("[$case] and finds the class", $shutdown['exists'] ?? null);
            continue;
        }

        $before = OxphpAutoloadGuard::calls($class);
        $next = json_decode(
            fiber_inner_request("/tests/fibers/fixture_autoload_guard.php?role=lookup&cls=$class", 8.0),
            true
        );
        $t->meta("{$case}_next", $next);
        $t->assertSame("[$case] nothing but the failing lookup reached the autoloader before the next request", $before, 1);
        $t->assertSame(
            "[$case] a lookup in the next request reaches the autoloader again",
            OxphpAutoloadGuard::calls($class),
            2
        );
        $t->assertTrue("[$case] and finds the class", $next['exists'] ?? null);
    }
}

$t->done();
