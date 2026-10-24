<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// Follows fibers/test_oom_in_a_superglobal_destructor_spares_the_next_request,
// whose $_GET held an object that runs out of memory in its destructor, with
// nothing else in flight on the worker. This request is the next one the worker
// starts, and it runs as if nothing had happened: before, it was handed the
// engine's note that it had reported running out of memory, and the exit() that
// unwound the destructor, and never got to run.

$t = new TestCase('oom_in_a_superglobal_destructor_spares_the_next_request_probe', 'fibers');

$base = sys_get_temp_dir() . '/oxphp-superglobal-oom-next';

// What the rest stands on: the destructor ran after the request before this one
// had answered.
$t->assertTrue('the destructor of what the request before left in $_GET ran', is_file("$base.ran"));

$t->assertSame(
    'a callback runs in the request after it',
    array_map(static fn (int $n): int => $n * 2, [1, 2]),
    [2, 4]
);

// On the worker that served that request, not on one started to replace it.
$t->assertSame(
    'on the worker that served the request before',
    sprintf('%.6f', OxPHP\Server\Worker::current()->startTime()),
    is_file("$base.worker") ? file_get_contents("$base.worker") : 'no record'
);

foreach (["$base.ran", "$base.worker"] as $file) {
    if (is_file($file)) {
        unlink($file);
    }
}

$t->done();
