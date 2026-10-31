<?php

declare(strict_types=1);

// Included by test_pool_callback_made_from_a_function_runs_as_it_on_another_thread
// after it declared the parent with eval(). OPcache keeps this file in shared
// memory, but a class whose parent is not there is linked by the request that
// includes it, into a copy of its own, its methods included.
final class AsyncDeclMadeFromLateChild extends AsyncDeclMadeFromEvalParent
{
    public static function counted(): object
    {
        static $n = 0;
        return (object) ['n' => ++$n];
    }

    public static function plain(): object
    {
        return (object) ['n' => 0];
    }

    public static function made(): AsyncDeclMadeFromLateTarget
    {
        return new AsyncDeclMadeFromLateTarget();
    }
}

final class AsyncDeclMadeFromLateTarget
{
    public int $v = 16;
}
