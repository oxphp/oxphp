<?php

/**
 * A named call may skip an optional parameter of a plugin function or method:
 * the engine fills the gap from the default the parameter declares, and
 * Reflection reports that default. Covers the method path (Shared\Pool,
 * Shared\Atomic) and the free-function path (APM, async) separately, since
 * each registers its arginfo on its own.
 */

require_once __DIR__ . '/../test_helper.php';

use OxPHP\Shared\Atomic;
use OxPHP\Shared\Ordering;
use OxPHP\Shared\Pool;

$t = new TestCase('named_args_skip_optional', 'sapi');

$attempt = function (string $name, callable $body) use ($t): void {
    try {
        $body();
    } catch (\Throwable $e) {
        $t->assertTrue($name . ': ' . get_class($e) . ': ' . $e->getMessage(), false);
    }
};

// Method: `destroy` skipped, `maxSize` named. The pool must not only be built
// but hold exactly one slot, so `maxSize: 1` landed in its own position.
$attempt('Pool skip destroy', function () use ($t): void {
    $pool = new Pool(factory: static fn (): \stdClass => new \stdClass(), maxSize: 1);
    $h = $pool->acquire();
    $t->assertNull('Pool maxSize: 1 is the capacity', $pool->tryAcquire());
    $h->release();
});

$attempt('Pool::__construct defaults', function () use ($t): void {
    $params = (new \ReflectionMethod(Pool::class, '__construct'))->getParameters();
    $t->assertFalse('factory has no default', $params[0]->isDefaultValueAvailable());
    $t->assertNull('destroy defaults to null', $params[1]->getDefaultValue());
    $t->assertSame('maxSize defaults to 32', $params[2]->getDefaultValue(), 32);
    $t->assertSame('idleTimeoutMs defaults to 300000', $params[3]->getDefaultValue(), 300000);
});

// Method with a constant-expression default: the engine evaluates
// `\OxPHP\Shared\Ordering::SeqCst` for the skipped `success`.
$attempt('Atomic::compareAndSet skip success', function () use ($t): void {
    $a = new Atomic(0);
    $t->assertTrue(
        'compareAndSet(expect, new, failure:) swaps',
        $a->compareAndSet(expect: 0, new: 1, failure: Ordering::Relaxed),
    );
    $t->assertSame('value after swap', $a->load(), 1);
    $params = (new \ReflectionMethod(Atomic::class, 'compareAndSet'))->getParameters();
    $t->assertSame('success defaults to SeqCst', $params[2]->getDefaultValue(), Ordering::SeqCst);
});

// Free function: `description` skipped, `span_id` named.
$attempt('oxphp_apm_status skip description', function () use ($t): void {
    oxphp_apm_status(0, span_id: 0);
    $params = (new \ReflectionFunction('oxphp_apm_status'))->getParameters();
    $t->assertNull('description defaults to null', $params[1]->getDefaultValue());
});

// A float default stays a float rather than reading back as an int.
$attempt('oxphp_async_await timeout default', function () use ($t): void {
    $params = (new \ReflectionFunction('oxphp_async_await'))->getParameters();
    $t->assertSame('timeout defaults to 0.0', $params[1]->getDefaultValue(), 0.0);
});

$t->done();
