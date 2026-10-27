<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// A task runs with a copy of its closure's variables. A `static` a call has
// set is held by reference, and the engine reads one whose initializer the
// compiler did not fold to a literal as a reference without checking: a copy
// holding the plain value would take the whole process down on reaching that
// `static`.
$t = new TestCase('test_task_closure_static_set_before_dispatch', 'asyncdecl');

// Under OPcache, on in this profile, the compiler folds neither a call nor a
// constant the script defines. The task's thread does not have this constant,
// so the task fails if it runs the initializer again.
const ASYNCDECL_STATIC_START = 0;

$called = static function (): int {
    static $made = random_int(0, 0);
    return ++$made;
};
$fromConstant = static function (): int {
    static $made = ASYNCDECL_STATIC_START;
    return ++$made;
};
$literal = static function (): int {
    static $made = 0;
    return ++$made;
};
foreach ([
    'a static initialized by a call' => $called,
    'a static initialized from a constant' => $fromConstant,
    'a static initialized with a literal' => $literal,
] as $what => $f) {
    $f();
    $t->assertSame("$what: the task counts on from the request's call", asyncdecl_settle(oxphp_async($f)), [true, 2]);
    $t->assertSame("$what: the request's closure counts on from its own call", $f(), 2);
}

// Set to null, a `static` keeps that value: its initializer does not run again.
$nulled = static function (): string {
    static $v = random_int(1, 1);
    $was = $v === null ? 'null' : "initialized to $v";
    $v = null;
    return $was;
};
$nulled();
$t->assertSame('a static set to null: the task sees null', asyncdecl_settle(oxphp_async($nulled)), [true, 'null']);

$appended = static function (): string {
    static $log = random_int(0, 0) === 0 ? 'a' : '';
    $log .= 'b';
    return $log;
};
$appended();
$t->assertSame('a string static: the task appends to it', asyncdecl_settle(oxphp_async($appended)), [true, 'abb']);
$t->assertSame("a string static: the request's closure appends to its own", $appended(), 'abb');

$fresh = static function (): int {
    static $made = random_int(0, 0);
    return ++$made;
};
$t->assertSame('a static not set yet: the task runs its initializer', asyncdecl_settle(oxphp_async($fresh)), [true, 1]);

// The engine hands out a `static` it holds the only reference to as a copy of
// its value, and Closure::getCurrent(), PHP 8.5 on, lets a task reach its own
// closure.
if (PHP_VERSION_ID >= 80500) {
    $reflected = static function (): int {
        $copy = (new \ReflectionFunction(\Closure::getCurrent()))->getStaticVariables();
        $copy['made'] = 5;
        static $made = 0;
        return ++$made;
    };
    $reflected();
    $t->assertSame("getStaticVariables() in the task: a copy, as in the request", asyncdecl_settle(oxphp_async($reflected)), [true, 2]);
}

// A variable captured by value stays a value, and one captured by reference a
// reference, of the task's own.
$x = 1;
$byValue = static function () use ($x): array {
    $x++;
    return [$x, \ReflectionReference::fromArrayElement(get_defined_vars(), 'x') !== null];
};
$byReference = static function () use (&$x): array {
    $x++;
    return [$x, \ReflectionReference::fromArrayElement(get_defined_vars(), 'x') !== null];
};
$t->assertSame('captured by value: a value in the task', asyncdecl_settle(oxphp_async($byValue)), [true, [2, false]]);
$t->assertSame('captured by reference: a reference in the task', asyncdecl_settle(oxphp_async($byReference)), [true, [2, true]]);
$t->assertSame("captured by reference: the request's variable is its own", $x, 1);

$t->done();
