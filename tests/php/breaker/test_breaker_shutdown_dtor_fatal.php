<?php

declare(strict_types=1);

// The other end a shutdown function can come apart at: not the call, but the
// free. Destroying the registry runs the destructors of everything the
// registered entries were holding, and a destructor can fatal — that bailout is
// swallowed by php_free_shutdown_functions' own zend_catch, one step further
// from the worker than the call itself.
//
// The object is held by nothing but the closure, so its destructor cannot run
// before the entry holding it is destroyed. The shutdown function itself does
// nothing, which is the point: if this file retires a worker, the free is the
// only thing that could have done it.
//
// The fatal is a redeclaration reached through require, not a `class` statement
// written here — see breaker_redeclare.php for why a method cannot contain one.
// E_COMPILE_ERROR is never routed to a user error handler, so it does not depend
// on what an earlier request on this worker left installed.
//
// The class is guarded rather than declared outright, because in worker mode it
// outlives its request and an unguarded second declaration would fatal as the
// file is included — which is a fatal in the handler, a case already covered
// above.
//
// No TestCase: this request has nothing to report.

if (!class_exists('BreakerShutdownDtorFatal', false)) {
    class BreakerShutdownDtorFatal
    {
        public function __destruct()
        {
            require __DIR__ . '/breaker_redeclare.php';
            require __DIR__ . '/breaker_redeclare.php';

            echo "unreachable: requiring the same declaration twice must be fatal\n";
        }
    }
}

$held = new BreakerShutdownDtorFatal();

register_shutdown_function(static function () use ($held): void {
    // Deliberately empty. What matters is that this closure holds $held until
    // the registry is freed.
});

// The closure's bound copy is now the only reference.
unset($held);

echo "the handler itself ends without incident\n";
