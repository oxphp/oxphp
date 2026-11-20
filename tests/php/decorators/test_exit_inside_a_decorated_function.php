<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

$t = new TestCase('exit_inside_a_decorated_function', 'decorators');

// exit() inside a decorated function unwinds its frame with the engine's
// UnwindExit, which is not a Throwable: the decorator's end handler must not
// read it as one. The request ends as exit() ends any request — the shutdown
// function below runs and answers — and the call is not reported to after(),
// which the engine does not call while that object is pending. Read as a
// Throwable, that object takes the whole server down, and the request gets no
// answer.

#[\Attribute(\Attribute::TARGET_FUNCTION)]
class ExitProbeDecorator implements \OxPHP\Decorator\AttributeInterface
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

oxphp_register_decorator(ExitProbeDecorator::class);

#[ExitProbeDecorator]
function exit_probe_leaves(): void
{
    ExitProbeDecorator::$log[] = 'body';
    exit();
}

register_shutdown_function(static function () use ($t): void {
    $t->assertSame(
        'the body ran to its exit(), and after() was not called',
        ExitProbeDecorator::$log,
        ['before', 'body']
    );
    $t->done();
});

exit_probe_leaves();
