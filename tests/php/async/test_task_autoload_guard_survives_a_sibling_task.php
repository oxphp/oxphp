<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// An oxphp_async() task suspended inside an autoloader keeps the engine's guard
// against loading that class again from inside its own autoloader, while
// another task starts on the same async worker.
//
// The engine keeps one table per thread of the class names its autoloaders are
// loading, and a lookup that finds the name there answers "not found" without
// calling them. This profile has one async worker, so both tasks run on one
// thread: the first suspends inside its autoloader, and the second starts while
// it is there. Starting a task must not empty the table under the suspended
// one — woken, its lookup of the class from inside its own autoloader would
// reach the autoloader again.
//
// The other side of it: the name the suspended task left in the table is not
// the sibling's. A sibling that looks the same class up while the holder is
// suspended reaches the autoloader and gets the class, where a table shared by
// every task on the thread would tell it the class does not exist.

$t = new TestCase('task_autoload_guard_survives_a_sibling_task', 'async');
$class = 'OxphpTaskAutoloadHeld' . bin2hex(random_bytes(4));

$first = oxphp_async(function (string $class): array {
    $calls = 0;
    $seen = [];
    $loader = function (string $name) use ($class, &$calls, &$seen): void {
        if (strcasecmp($name, $class) !== 0) {
            return;
        }
        $calls++;
        if ($calls === 1) {
            $seen['suspended_at'] = hrtime(true);
            oxphp_sleep(0.2);
            $seen['woke_at'] = hrtime(true);
            $before = $calls;
            $seen['found_inside'] = class_exists($class);
            $seen['reentered'] = $calls !== $before;
        }
        // The name is made of letters and digits only.
        if (!class_exists($class, false)) {
            eval("final class $class {}");
        }
    };
    spl_autoload_register($loader);
    try {
        $seen['exists'] = class_exists($class);
    } finally {
        spl_autoload_unregister($loader);
    }

    return $seen;
}, $class);
$second = oxphp_async(static fn (): int => hrtime(true));

$ranAt = oxphp_async_await($second);
$seen = oxphp_async_await($first);
$t->meta('first', $seen);

$t->assertTrue(
    'the second task ran while the first was suspended inside its autoloader',
    isset($seen['suspended_at'], $seen['woke_at']) && $seen['suspended_at'] < $ranAt && $ranAt < $seen['woke_at']
);
$t->assertSame(
    'woken, the first task\'s lookup of the class from inside its autoloader did not reach it again',
    $seen['reentered'] ?? null,
    false
);
$t->assertSame('and did not find the class', $seen['found_inside'] ?? null, false);
$t->assertTrue('the first task found the class once its autoloader returned', $seen['exists'] ?? null);

$shared = 'OxphpTaskAutoloadShared' . bin2hex(random_bytes(4));
$holder = oxphp_async(function (string $class): array {
    $calls = 0;
    $seen = [];
    $loader = function (string $name) use ($class, &$calls, &$seen): void {
        if (strcasecmp($name, $class) !== 0) {
            return;
        }
        $calls++;
        if ($calls === 1) {
            $seen['suspended_at'] = hrtime(true);
            oxphp_sleep(0.2);
            $seen['woke_at'] = hrtime(true);
        }
        if (!class_exists($class, false)) {
            eval("final class $class {}");
        }
    };
    spl_autoload_register($loader);
    try {
        $seen['exists'] = class_exists($class);
    } finally {
        spl_autoload_unregister($loader);
    }
    $seen['calls'] = $calls;

    return $seen;
}, $shared);
// Autoloaders are the thread's, so the sibling's lookup goes through the
// holder's.
$sibling = oxphp_async(static fn (string $class): array => ['at' => hrtime(true), 'exists' => class_exists($class)], $shared);

$siblingSeen = oxphp_async_await($sibling);
$holderSeen = oxphp_async_await($holder);
$t->meta('holder', $holderSeen);
$t->meta('sibling', $siblingSeen);

$t->assertTrue(
    'the sibling looked the class up while the holder was suspended inside its autoloader',
    isset($holderSeen['suspended_at'], $holderSeen['woke_at'], $siblingSeen['at'])
        && $holderSeen['suspended_at'] < $siblingSeen['at'] && $siblingSeen['at'] < $holderSeen['woke_at']
);
$t->assertTrue('the sibling found the class', $siblingSeen['exists'] ?? null);
$t->assertSame('its lookup reached the autoloader', $holderSeen['calls'] ?? null, 2);
$t->assertTrue('the holder found the class too once let go', $holderSeen['exists'] ?? null);

$t->done();
