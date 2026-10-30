<?php

declare(strict_types=1);

require_once __DIR__ . '/../test_helper.php';

// An oxphp_async() task runs in a fiber the server drives, so a userland
// Fiber::suspend() on it is refused with a FiberError the task can catch, as it
// is for a request fiber in worker mode. The refusal is made on the async
// worker's own thread, between the task's slices, where no PHP frame is on the
// stack: the error has to reach the task without being thrown there.

$t = new TestCase('task_fiber_suspend_is_refused', 'async');

// Caught, twice, and the fiber can still park and be resumed afterwards.
$p = oxphp_async(static function (): array {
    $refusals = [];
    for ($i = 0; $i < 2; $i++) {
        try {
            \Fiber::suspend('x');
            $refusals[] = 'resumed';
        } catch (\Throwable $e) {
            $refusals[] = get_class($e) . ': ' . $e->getMessage();
        }
    }
    $before = microtime(true);
    oxphp_sleep(0.02);

    return ['refusals' => $refusals, 'slept' => microtime(true) - $before];
});
$r = oxphp_async_await($p, 5.0);
$t->assertCount('caught: both attempts reached the task', $r['refusals'], 2);
$t->assertContains('caught: the first is a FiberError naming a task fiber', $r['refusals'][0], 'FiberError: Cannot suspend an OxPHP task fiber');
$t->assertContains('caught: the second is refused too', $r['refusals'][1], 'FiberError: Cannot suspend an OxPHP task fiber');
$t->assertGreaterThan('caught: the task can still park and be resumed', $r['slept'], 0.015);

// Not caught: the refusal is the task's outcome.
$p = oxphp_async(static function (): string {
    \Fiber::suspend();

    return 'resumed';
});
$message = null;
try {
    oxphp_async_await($p, 5.0);
} catch (\OxPHP\Async\AsyncException $e) {
    $message = $e->getMessage();
}
$t->assertNotNull('uncaught: the awaiter gets an AsyncException', $message);
$t->assertContains('uncaught: it carries the refusal', (string) $message, 'Cannot suspend an OxPHP task fiber');

// Caught and retried without end: the task is ended rather than argued with,
// its finally blocks still run, and its awaiter is told so instead of being
// handed a result the task never returned.
$marker = sys_get_temp_dir() . '/oxphp-task-suspend-retry-' . bin2hex(random_bytes(4));
$p = oxphp_async(static function (string $marker): string {
    try {
        while (true) {
            try {
                \Fiber::suspend();
            } catch (\FiberError) {
            }
        }
    } finally {
        file_put_contents($marker, 'ran');
    }
}, $marker);
$message = null;
$value = 'not awaited';
try {
    $value = oxphp_async_await($p, 5.0);
} catch (\OxPHP\Async\AsyncException $e) {
    $message = $e->getMessage();
}
$t->assertNotNull('retried: the awaiter gets an AsyncException, not a value', $message);
$t->assertContains('retried: it says the task was ended', (string) $message, 'kept retrying');
$t->assertSame('retried: no value was delivered', $value, 'not awaited');
$t->assertTrue('retried: the finally block ran', file_exists($marker));
if (file_exists($marker)) {
    unlink($marker);
}

// Refused inside a finally block that an exception is unwinding through: the
// FiberError takes that exception as its previous, so releasing the FiberError
// releases it too, and with it the generator its trace holds as an argument.
// Destroying the generator runs its finally block wherever that release happens.
// It has to be in the task's fiber — on the async worker's own thread there is
// no frame to throw into, and the throw below would end the process. (A userland
// class does not exist on that thread, hence a generator for the destructor.)
// With exception arguments ignored the trace holds nothing, unset() destroys the
// generator before the refusal, and the marker passes on any build.
$t->assertFalse('chained: zend.exception_ignore_args is off', (bool) ini_get('zend.exception_ignore_args'));
$marker = sys_get_temp_dir() . '/oxphp-task-suspend-chained-' . bin2hex(random_bytes(4));
$p = oxphp_async(static function (string $marker): string {
    $gen = (static function () use ($marker): \Generator {
        try {
            yield 1;
        } finally {
            file_put_contents($marker, \Fiber::getCurrent() !== null ? 'in a fiber' : 'outside every fiber');
            throw new \LogicException('thrown from a generator being destroyed');
        }
    })();
    $gen->current();
    $throw = static function (\Generator $held): never {
        throw new \RuntimeException('unwinding');
    };
    try {
        try {
            $throw($gen);
        } finally {
            unset($gen);
            \Fiber::suspend();
        }
    } catch (\FiberError) {
        return 'caught';
    }
}, $marker);
try {
    oxphp_async_await($p, 5.0);
} catch (\OxPHP\Async\AsyncException) {
}
$t->assertSame('chained: the generator was destroyed in the task\'s fiber', file_exists($marker) ? file_get_contents($marker) : 'not destroyed', 'in a fiber');
if (file_exists($marker)) {
    unlink($marker);
}

// Retried by what the task left behind after it returned or threw: a generator
// held by a shutdown function is destroyed as the task's registrations are
// freed, and its finally block keeps retrying. That code is ended too, and the
// task keeps the outcome it had.
foreach (['returned' => 'returned by the task', 'threw' => 'thrown by the task'] as $ending => $expected) {
    $marker = sys_get_temp_dir() . '/oxphp-task-suspend-after-end-' . bin2hex(random_bytes(4));
    $p = oxphp_async(static function (string $marker, string $ending): string {
        $gen = (static function () use ($marker): \Generator {
            try {
                yield 1;
            } finally {
                $attempts = 0;
                try {
                    for (; $attempts < 100; $attempts++) {
                        try {
                            \Fiber::suspend();
                        } catch (\FiberError) {
                        }
                    }
                } finally {
                    file_put_contents($marker, (string) $attempts);
                }
            }
        })();
        $gen->current();
        register_shutdown_function(static function () use ($gen): void {
        });
        if ($ending === 'threw') {
            throw new \RuntimeException('thrown by the task');
        }

        return 'returned by the task';
    }, $marker, $ending);
    $outcome = null;
    try {
        $outcome = oxphp_async_await($p, 5.0);
    } catch (\OxPHP\Async\AsyncException $e) {
        $outcome = $e->getMessage();
    }
    $t->assertSame("{$ending}: the retrying left behind was ended", file_exists($marker) ? file_get_contents($marker) : 'did not run', '32');
    $t->assertContains("{$ending}: the task keeps its outcome", (string) $outcome, $expected);
    if (file_exists($marker)) {
        unlink($marker);
    }
}

$t->assertSame('the next task runs', oxphp_async_await(oxphp_async(static fn (): int => 42), 5.0), 42);

$t->done();
