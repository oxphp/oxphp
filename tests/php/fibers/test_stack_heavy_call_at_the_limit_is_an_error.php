<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('stack_heavy_call_at_the_limit_is_an_error', 'fibers');

// The engine checks the C stack when internal code calls back into PHP code —
// as array_map does on every level here — not on every byte an internal
// function puts there afterwards, and it keeps zend.reserved_stack_size free
// below the limit for exactly that. realpath() is such a function: on a
// thread-safe build it holds three path buffers of MAXPATHLEN on the stack at
// once, about 12 KiB.
//
// Each level calls it before it descends, so the last call runs on the deepest
// level that passed the check — as close to the limit as the recursion gets.
// With the reserve below the limit that is an ordinary Error the request or the
// task catches. Without it, realpath() runs into the guard page under the
// fiber's stack and the whole server dies.
//
// The task runs on the async worker thread, which does not see anything this
// request declares, so the recursion is spelled out again inside it.
$task = oxphp_async(static function (): array {
    $expected = realpath(__DIR__);
    $deepest = 0;
    $differed = 0;
    $descend = static function (int $depth) use (&$descend, &$deepest, &$differed, $expected): array {
        if (realpath(__DIR__) !== $expected) {
            $differed++;
        }
        $deepest = $depth;

        return array_map($descend, [$depth + 1]);
    };
    try {
        $descend(0);
        $message = 'no error';
    } catch (\Error $e) {
        $message = $e->getMessage();
    }

    return ['message' => $message, 'deepest' => $deepest, 'differed' => $differed];
});

$expected = realpath(__DIR__);
$deepest = 0;
$differed = 0;
$descend = static function (int $depth) use (&$descend, &$deepest, &$differed, $expected): array {
    if (realpath(__DIR__) !== $expected) {
        $differed++;
    }
    $deepest = $depth;

    return array_map($descend, [$depth + 1]);
};
try {
    $descend(0);
    $message = 'no error';
} catch (\Error $e) {
    $message = $e->getMessage();
}

$results = [
    'request' => ['message' => $message, 'deepest' => $deepest, 'differed' => $differed],
    'async task' => oxphp_async_await($task, 30.0),
];
$t->meta('deepest', array_map(static fn (array $r): int => $r['deepest'], $results));

foreach ($results as $where => $r) {
    $t->assertMatch("the $where recursion was stopped by the C-stack check", $r['message'], '/^Maximum call stack size of \d+ bytes/');
    $t->assertGreaterThan("realpath ran on the levels of the $where recursion", $r['deepest'], 0);
    $t->assertSame("realpath answered the same on every level of the $where recursion", $r['differed'], 0);
}

$t->done();
