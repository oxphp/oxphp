<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('stack_limit_is_the_engines', 'fibers');

// A request runs on the C stack of its own fiber, and recursion that passes
// through an internal function on every level (array_map calling back into
// userland) grows that stack until the engine throws "Maximum call stack size
// of N bytes". N is the distance between the top of the stack and the limit
// the engine checks against, and the limit has to sit zend.reserved_stack_size
// above the guard page: internal code keeps using the stack after the last
// check, and only that reserve keeps it off the guard page.
//
// The engine sets that limit itself for every fiber it starts. A userland
// \Fiber started here gets exactly that, so its N is the reference — whatever
// the page size, fiber.stack_size or reserve of this build — and the request's
// fiber and an async task's fiber must report the same.
$probe = static function (): string {
    $descend = static function (int $depth) use (&$descend): array {
        return array_map($descend, [$depth + 1]);
    };
    try {
        $descend(0);
    } catch (\Error $e) {
        return $e->getMessage();
    }

    return 'no error';
};

// The task runs on the async worker thread, which does not see anything this
// request declares, so the probe is spelled out again inside it.
$task = oxphp_async(static function (): string {
    $descend = static function (int $depth) use (&$descend): array {
        return array_map($descend, [$depth + 1]);
    };
    try {
        $descend(0);
    } catch (\Error $e) {
        return $e->getMessage();
    }

    return 'no error';
});

$fiber = new \Fiber($probe);
$fiber->start();

$messages = [
    'request' => $probe(),
    'userland fiber' => $fiber->getReturn(),
    'async task' => oxphp_async_await($task, 30.0),
];

$sizes = [];
foreach ($messages as $where => $message) {
    // The premise: each recursion was stopped by the C-stack check, not by
    // memory_limit or anything else that throws on the way down.
    $t->assertMatch("the $where recursion reached the C-stack limit", $message, '/^Maximum call stack size of \d+ bytes/');
    $sizes[$where] = preg_match('/^Maximum call stack size of (\d+) bytes/', $message, $m) === 1 ? (int) $m[1] : null;
}
$t->meta('sizes', $sizes);

$t->assertSame('the request fiber has the limit the engine gives a fiber', $sizes['request'], $sizes['userland fiber']);
$t->assertSame('an async task fiber has the limit the engine gives a fiber', $sizes['async task'], $sizes['userland fiber']);

$t->done();
