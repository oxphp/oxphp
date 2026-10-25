<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// What a task returned, and the closure it ran, are released on the async
// worker's own thread once the task has finished, between tasks, with no PHP
// frame there. Releasing them can run userland — the finally block of a
// generator, a destructor — and that userland can end in a fatal error, or in
// an exception that, with no frame to catch it, the engine turns into one. The
// awaiter still gets what the task produced, the userland runs, and the worker
// goes on to the next task.
//
// The userland writes a file before it fails, so the test can tell it ran.

$t = new TestCase('task_payload_release_bailout', 'async');

$run = bin2hex(random_bytes(4));
$marker = sys_get_temp_dir() . "/oxphp-task-payload-release-$run";

// The task returns a generator suspended inside try, whose finally throws.
$p = oxphp_async(static function (string $marker): \Generator {
    $g = (static function () use ($marker): \Generator {
        try {
            yield 1;
        } finally {
            file_put_contents("$marker-retval", 'ran');
            throw new \RuntimeException('finally of a task result');
        }
    })();
    $g->current();

    return $g;
}, $marker);
try {
    oxphp_async_await($p);
} catch (\Throwable) {
    // A generator cannot cross threads. What the awaiter gets for one is not
    // what this test is about.
}
$t->assertTrue('retval: the finally block ran', file_exists("$marker-retval"));
$t->assertSame('retval: the next task runs', oxphp_async_await(oxphp_async(static fn (): int => 42)), 42);

// The task keeps such a generator in a static variable of its own closure, and
// returns a value that does cross.
$p = oxphp_async(static function (string $marker): int {
    static $keep;
    $keep = (static function () use ($marker): \Generator {
        try {
            yield 1;
        } finally {
            file_put_contents("$marker-closure", 'ran');
            throw new \RuntimeException('finally of a task closure');
        }
    })();
    $keep->current();

    return 7;
}, $marker);
$t->assertSame('closure: the awaiter gets the value', oxphp_async_await($p), 7);
$t->assertTrue('closure: the finally block ran', file_exists("$marker-closure"));
$t->assertSame('closure: the next task runs', oxphp_async_await(oxphp_async(static fn (): int => 42)), 42);

// The finally block raises a fatal error instead. A compile error, which no
// error handler sees: the engine bails out on the spot. A generator rather than
// an object with a destructor: a class this script declares does not exist on
// the async worker's thread.
$p = oxphp_async(static function (string $marker): \Generator {
    $g = (static function () use ($marker): \Generator {
        try {
            yield 1;
        } finally {
            file_put_contents("$marker-fatal", 'ran');
            eval('abstract class OxphpTaskPayloadFatal { abstract public function f() {} }');
        }
    })();
    $g->current();

    return $g;
}, $marker);
try {
    oxphp_async_await($p);
} catch (\Throwable) {
}
$t->assertTrue('fatal: the finally block ran', file_exists("$marker-fatal"));
$t->assertSame('fatal: the next task runs', oxphp_async_await(oxphp_async(static fn (): int => 42)), 42);

$t->done();
