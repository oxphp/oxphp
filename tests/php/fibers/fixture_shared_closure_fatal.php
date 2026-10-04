<?php

declare(strict_types=1);

// Inner self-request for fibers/test_shared_closure_fatal_releases.
//
// A fatal raised inside a callable that a Shared\* method runs on the caller's
// behalf: the closure of Mutex::withLock and tryWithLock, the factory of
// Once::getOrInit, of a Registry get-or-create and of a Pool, the body of
// Pool::with, the callback of Map::forEach. While the callable runs, every one
// of these methods but Map::forEach holds something — the lock, the cell's init
// lock, the key's creation slot, a slot of the pool or the budget for one — and
// it gives that back when the callable returns or throws. A fatal does neither: it ends the request from
// inside the callable, and what the method held has to be given back on the
// fatal's way out, or it stays held for the life of the process. The same goes
// for a fatal just around the callable while the method holds it: from the
// autoloader that resolving a named callable runs, or from the destructor of a
// value the callable returned and the method could not store.
//
// Every primitive is bound in the Registry under the run id the test passes, so
// the test can reach the same ones once this request is over, and a second run
// against the same server starts from fresh ones.

use OxPHP\Shared\Counter;
use OxPHP\Shared\Map;
use OxPHP\Shared\Mutex;
use OxPHP\Shared\Once;
use OxPHP\Shared\Once\FailureMode;
use OxPHP\Shared\Pool;
use OxPHP\Shared\Registry;

$run = (string) ($_GET['run'] ?? '');
$case = (string) ($_GET['case'] ?? '');

// A compile error rather than E_USER_ERROR: this worker keeps the error handler
// an earlier request installed, and that handler turns E_USER_ERROR into an
// exception, which is the path a throw takes and not the one under test. No
// handler sees E_COMPILE_ERROR. The compile fails before the class is bound, so
// nothing is declared and the next run can do the same.
$fatal = static function (): void {
    eval('abstract class OxphpSharedClosureFatal { abstract public function f() {} }');
};

// A value a callable can return that the method cannot store, and whose
// destructor raises that fatal: the method frees it once the callable has
// returned, with what it holds still held.
$fatalOnDestruct = static fn (): object => new class ($fatal) {
    public function __construct(private \Closure $fatal)
    {
    }

    public function __destruct()
    {
        ($this->fatal)();
    }
};

// Named rather than a closure so the test can take it off again: the worker
// keeps an autoloader across requests, and a declared function with it.
if (!function_exists('oxphp_shared_lazy_fatal_autoload')) {
    function oxphp_shared_lazy_fatal_autoload(string $class): void
    {
        if ($class === 'OxphpSharedLazyFatal') {
            eval('abstract class OxphpSharedLazyFatal { abstract public static function make() {} }');
        }
    }
}

// A fatal cannot be caught. One the method swallowed on its way out would come
// back as the exception the method throws instead, and land here.
try {
    switch ($case) {
        case 'with_lock':
            $m = Registry::mutex("closure-fatal-with-lock-$run", static fn (): Mutex => new Mutex('clean'));
            $m->withLock(static function (mixed &$v) use ($fatal): void {
                $v = 'dirty';
                $fatal();
            });
            break;

        case 'with_lock_parked':
            // The time limit runs out while the closure is parked in a native
            // sleep rather than running: the request is ended from there.
            $m = Registry::mutex("closure-fatal-with-lock-parked-$run", static fn (): Mutex => new Mutex('clean'));
            set_time_limit(1);
            $m->withLock(static function (mixed &$v): void {
                $v = 'dirty';
                usleep(3_000_000);
            });
            break;

        case 'with_lock_destructor':
            // The closure writes and returns, and what it returns cannot be
            // stored: the fatal comes from its destructor, after the closure
            // has finished and while the lock is still held.
            $m = Registry::mutex("closure-fatal-with-lock-destructor-$run", static fn (): Mutex => new Mutex('clean'));
            $m->withLock(static function (mixed &$v) use ($fatalOnDestruct): object {
                $v = 'dirty';
                return $fatalOnDestruct();
            });
            break;

        case 'try_with_lock':
            $m = Registry::mutex("closure-fatal-try-with-lock-$run", static fn (): Mutex => new Mutex('clean'));
            $m->tryWithLock(static function (mixed &$v) use ($fatal): void {
                $v = 'dirty';
                $fatal();
            });
            break;

        case 'once':
            // Poison, because that is the mode in which the cell would otherwise be
            // left terminal: a fatal is not the factory failing, it is the request
            // being ended, and the next caller has to be able to run it again.
            $o = Registry::once("closure-fatal-once-$run", static fn (): Once => new Once(FailureMode::Poison));
            $o->getOrInit($fatal);
            break;

        case 'once_autoload':
            // The factory is named rather than given, and naming it loads its
            // class: the fatal comes from the autoloader, before any factory
            // has run, with the cell's init lock already taken.
            spl_autoload_register('oxphp_shared_lazy_fatal_autoload');
            $o = Registry::once("closure-fatal-once-autoload-$run", static fn (): Once => new Once(FailureMode::Poison));
            $o->getOrInit(['OxphpSharedLazyFatal', 'make']);
            break;

        case 'registry':
            Registry::counter("closure-fatal-registry-$run", $fatal);
            break;

        case 'registry_destructor':
            // The factory returns, and what it returns cannot be stored: the
            // method frees it, and the fatal comes from its destructor, after
            // the factory has finished.
            Registry::counter("closure-fatal-registry-destructor-$run", $fatalOnDestruct);
            break;

        case 'pool_factory':
            // The factory fatals the first time it runs and only then, so the test
            // can have the pool create a slot afterwards.
            $calls = new Counter();
            $p = Registry::pool("closure-fatal-pool-factory-$run", static fn (): Pool => new Pool(
                factory: static function () use ($calls, $fatal): \stdClass {
                    if ($calls->add() === 1) {
                        $fatal();
                    }
                    return new \stdClass();
                },
                destroy: null,
                maxSize: 1,
            ));
            $p->tryAcquire();
            break;

        case 'pool_with':
            $p = Registry::pool("closure-fatal-pool-with-$run", static fn (): Pool => new Pool(
                factory: static fn (): \stdClass => new \stdClass(),
                destroy: null,
                maxSize: 1,
            ));
            $p->with($fatal);
            break;

        case 'for_each':
            // Holds nothing across the callback, so there is nothing to read back
            // afterwards; it is here for the fatal itself, which has to end the
            // request the same way from this callable as from the others.
            $map = new Map();
            $map->set('k', 1);
            $map->forEach($fatal);
            break;

        case 'none':
            // Raises nothing and completes: the test sends one of these after each
            // fatal, so the worker does not count the fatals as a run.
            echo "OK\n";
            return;

        default:
            echo "FAIL: unknown case\n";
            return;
    }
} catch (\Throwable $e) {
    echo 'CAUGHT: ', get_class($e), ': ', $e->getMessage(), "\n";
}

echo "NOT-REACHED\n";
