<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('async_declared_return_types', 'async');

// The declared return type of an internal function is a contract the engine
// builds on: the optimizer sizes JIT traces by it, and Reflection materializes
// it into a ReflectionType. A malformed type encoding therefore surfaces in
// two ways — hot JIT'd await loops leak their results (covered by
// test_await_jit_hot_loop_leak), and reading the type via Reflection crashes
// the worker outright. Asserting the printed form of every declared type
// covers the second manifestation and pins the encoding itself.

$expected = [
    'oxphp_async' => 'int',
    'oxphp_async_await' => 'mixed',
    'oxphp_async_await_all' => 'array',
    'oxphp_async_await_race' => 'array',
    'oxphp_async_await_any' => 'array',
];

foreach ($expected as $fn => $type) {
    $rt = (new ReflectionFunction($fn))->getReturnType();
    $t->assertSame(
        "$fn declares return type $type",
        $rt instanceof ReflectionType ? (string) $rt : null,
        $type
    );
}

// The arginfo builder splits on arity, not on function-vs-method: declarations
// with parameters build their return slot inline, zero-parameter ones delegate
// to a separate return-only builder. The five functions above all take
// parameters, so a zero-parameter declaration is what pins the second path —
// jsonSerialize() happens to be one.
$mrt = (new ReflectionMethod('OxPHP\\Async\\BorrowedProxy', 'jsonSerialize'))->getReturnType();
$t->assertSame(
    'BorrowedProxy::jsonSerialize declares return type mixed',
    $mrt instanceof ReflectionType ? (string) $mrt : null,
    'mixed'
);

$t->done();
