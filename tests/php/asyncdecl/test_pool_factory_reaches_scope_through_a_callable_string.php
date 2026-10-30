<?php

declare(strict_types=1);

require_once __DIR__ . '/helper.php';

function asyncdecl_callable_scope_helper(int $base = 0): int
{
    return $base + 1;
}

// A callable that names its class as self, static or parent, as a string or
// in an array, is resolved from the class of the function that calls it, not
// by a name this thread's tables must hold. A factory that holds such a
// callable has that class checked whole, as for self:: in its code. PHP
// deprecates these callables, and TestCase turns the notice into an exception
// whatever @ says, so each is called with deprecations ignored. Nothing else in
// these methods names the class that way.
$t = new TestCase('test_pool_factory_reaches_scope_through_a_callable_string', 'asyncdecl');

$makeLine = __LINE__ + 5;
class AsyncDeclCallableMaker
{
    public static function make(): object
    {
        return (object) ['v' => asyncdecl_callable_scope_helper()];
    }

    public static function staticString(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func('static::make');
        } finally {
            \restore_error_handler();
        }
    }

    // The method name is known only at run time, so the optimizer cannot fold
    // the callable into one 'static::make' literal: only 'static::' is one.
    public static function builtString(): object
    {
        $method = \array_rand(['make' => true]);
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func('static::' . $method);
        } finally {
            \restore_error_handler();
        }
    }

    public static function selfArray(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func(['self', 'make']);
        } finally {
            \restore_error_handler();
        }
    }
}

final class AsyncDeclCallableChild extends AsyncDeclCallableMaker
{
    public static function parentString(): object
    {
        \set_error_handler(static fn (): bool => true, E_DEPRECATED);
        try {
            return \call_user_func('parent::make');
        } finally {
            \restore_error_handler();
        }
    }
}

$refused = asyncdecl_refused('asyncdecl_callable_scope_helper', __FILE__, $makeLine);
$pools = [
    "'static::make'" => [AsyncDeclCallableMaker::class, 'staticString'],
    "'static::' . \$method" => [AsyncDeclCallableMaker::class, 'builtString'],
    "['self', 'make']" => [AsyncDeclCallableMaker::class, 'selfArray'],
    "'parent::make'" => [AsyncDeclCallableChild::class, 'parentString'],
];
foreach ($pools as $what => $factory) {
    $pool = new \OxPHP\Shared\Pool(factory: $factory, destroy: null, maxSize: 1);
    $t->assertSame("$what: in a task", asyncdecl_acquire_in_task($pool), $refused);
    $h = $pool->acquire();
    $t->assertSame("$what: on the worker thread the factory runs", $h->get()->v, 1);
    $h->release();
}

$t->done();
