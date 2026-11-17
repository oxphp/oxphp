<?php

declare(strict_types=1);

// A request parked inside an autoloader does not stop another request on the
// worker from loading the same class. The engine keeps one table per thread of
// the class names its autoloaders are loading, and a lookup that finds the name
// there answers "not found" without calling them. Shared by every request on the
// worker, the name of a class one request is still loading would make the
// lookup of every other request fail for as long as that one stays parked.
//
// The holder request parks inside the autoloader; this request looks the same
// class up while it is there, then lets it go.
//
// The other side of it: the holder keeps the class marked for its own lookups.
// A second holder parks inside the autoloader of a class nothing else looks up,
// while this request runs; let go, it looks the class up from inside its own
// autoloader, which must not reach the autoloader again.

require_once __DIR__ . '/../test_helper.php';
require_once __DIR__ . '/autoload_guard_probe.php';

$t = new TestCase('autoload_guard_while_parked', 'fibers');
$class = 'OxphpAutoloadGuardHeld' . bin2hex(random_bytes(4));
$key = strtolower($class);
OxphpAutoloadGuard::arm($class, 'hold');

$holder = thread_cell_send("/tests/fibers/fixture_autoload_guard.php?role=lookup&cls=$class");
$entered = thread_cell_wait_for(OxphpAutoloadGuard::$flags, "$key:in", 5.0);
// Read at the moment of the lookup: the holder is inside the autoloader and has
// not come out of it.
$held = $entered && !(OxphpAutoloadGuard::$flags["$key:out"] ?? false);
$found = class_exists($class);
$calls = OxphpAutoloadGuard::calls($class);

OxphpAutoloadGuard::$flags["$key:release"] = true;
$answer = json_decode(thread_cell_read($holder)['body'], true);
$t->meta('holder', $answer);

$t->assertTrue('the holder was parked inside the autoloader during the lookup', $held);
$t->assertSame('the lookup reached the autoloader', $calls, 2);
$t->assertTrue('and found the class', $found);
$t->assertTrue('the holder found it too once let go', $answer['exists'] ?? null);

$kept = 'OxphpAutoloadGuardKept' . bin2hex(random_bytes(4));
$keptKey = strtolower($kept);
OxphpAutoloadGuard::arm($kept, 'hold');

$keeper = thread_cell_send("/tests/fibers/fixture_autoload_guard.php?role=lookup&cls=$kept");
$keptEntered = thread_cell_wait_for(OxphpAutoloadGuard::$flags, "$keptKey:in", 5.0);
// This request runs here, on the worker, with the holder parked.
$keptHeld = $keptEntered && !(OxphpAutoloadGuard::$flags["$keptKey:out"] ?? false);

OxphpAutoloadGuard::$flags["$keptKey:release"] = true;
$keptAnswer = json_decode(thread_cell_read($keeper)['body'], true);
$t->meta('keeper', $keptAnswer);

$t->assertTrue('the second holder was parked inside the autoloader while this request ran', $keptHeld);
$t->assertSame(
    'once let go, its lookup of the class from inside its autoloader did not reach the autoloader again',
    OxphpAutoloadGuard::$flags["$keptKey:reentered"] ?? null,
    false
);
$t->assertSame('and did not find it', OxphpAutoloadGuard::$flags["$keptKey:found_inside"] ?? null, false);
$t->assertTrue('the second holder found the class once its autoloader returned', $keptAnswer['exists'] ?? null);

$t->done();
