<?php

declare(strict_types=1);

// require_once, not require: PHP_WORKERS=1 in this profile, so every test in it
// hits the same persistent worker and a bare require re-declares TestCase.
require_once __DIR__ . '/../test_helper.php';

// exit() inside a decorated function, in a request a worker serves: the frame is
// unwound with the engine's UnwindExit, which is not a Throwable, and the
// decorator's end handler must not read it as one. The request ends as exit()
// ends any request — its shutdown function runs and answers — and after() is not
// called, which the engine does not do while that object is pending. Read as a
// Throwable, that object takes the whole server down, and with it the worker and
// every test after this one.

$t = new TestCase('exit_inside_a_decorated_function_keeps_the_worker', 'fibers');

#[\Attribute(\Attribute::TARGET_FUNCTION)]
class WorkerExitProbeDecorator implements \OxPHP\Decorator\AttributeInterface
{
    public static array $log = [];

    public function before(\OxPHP\Decorator\Context $ctx): void
    {
        self::$log[] = 'before';
    }

    public function after(\OxPHP\Decorator\Context $ctx): void
    {
        self::$log[] = 'after';
    }
}

oxphp_register_decorator(WorkerExitProbeDecorator::class);

#[WorkerExitProbeDecorator]
function worker_exit_probe_leaves(): void
{
    WorkerExitProbeDecorator::$log[] = 'body';
    exit();
}

register_shutdown_function(static function () use ($t): void {
    $t->assertSame(
        'the body ran to its exit(), and after() was not called',
        WorkerExitProbeDecorator::$log,
        ['before', 'body']
    );
    $t->done();
});

worker_exit_probe_leaves();
