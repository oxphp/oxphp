<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

// The pool keeps a copy of a closure it is given, but not of the object a
// callback calls: an invokable object, an object whose __call() the factory
// reaches, and a closure that is the $this of the factory are the ones the
// code creating the pool passed.
$t = new TestCase('test_pool_callable_calls_the_object_it_was_given', 'asyncdecl');

final class AsyncDeclCountingMaker
{
    public int $calls = 0;

    public function __call(string $name, array $args): object
    {
        $this->calls++;
        return new \stdClass();
    }
}

final class AsyncDeclCountingInvokable
{
    public int $calls = 0;

    public function __invoke(): object
    {
        $this->calls++;
        return new \stdClass();
    }
}

$invokable = new AsyncDeclCountingInvokable();
$pool = new \OxPHP\Shared\Pool(factory: $invokable, destroy: null, maxSize: 1);
$pool->acquire()->release();
$t->assertSame('__invoke() runs on the object passed', $invokable->calls, 1);
unset($pool);

$maker = new AsyncDeclCountingMaker();
$pool = new \OxPHP\Shared\Pool(factory: [$maker, 'make'], destroy: null, maxSize: 1);
$pool->acquire()->release();
$t->assertSame('__call() runs on the object passed', $maker->calls, 1);
unset($pool);

$inner = static fn (): int => 1;
$factory = \Closure::bind(function (): object {
    return $this;
}, $inner);
$pool = new \OxPHP\Shared\Pool(factory: $factory, destroy: null, maxSize: 1);
$h = $pool->acquire();
$t->assertTrue('a factory bound to a closure returns that closure', $h->get() === $inner);
$h->release();
unset($h, $pool);

$t->done();
