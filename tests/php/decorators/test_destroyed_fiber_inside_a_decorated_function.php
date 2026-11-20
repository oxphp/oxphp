<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('destroyed_fiber_inside_a_decorated_function', 'decorators');

// A Fiber destroyed while it is suspended inside a decorated function is unwound
// by the engine with its GracefulExit, which is not a Throwable: the decorator's
// end handler must not read it as one. The script goes on past the destruction,
// and after() is not called, which the engine does not do while that object is
// pending. Read as a Throwable, that object takes the whole server down at the
// unset().

#[\Attribute(\Attribute::TARGET_FUNCTION)]
class FiberExitProbeDecorator implements \OxPHP\Decorator\AttributeInterface
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

oxphp_register_decorator(FiberExitProbeDecorator::class);

#[FiberExitProbeDecorator]
function fiber_exit_probe_parks(): void
{
    FiberExitProbeDecorator::$log[] = 'body';
    \Fiber::suspend();
    FiberExitProbeDecorator::$log[] = 'resumed';
}

$fiber = new \Fiber(static function (): void {
    fiber_exit_probe_parks();
});
$fiber->start();
unset($fiber);

$t->assertSame(
    'the body ran to its suspend, was never resumed, and after() was not called',
    FiberExitProbeDecorator::$log,
    ['before', 'body']
);
$t->done();
