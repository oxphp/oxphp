<?php

declare(strict_types=1);

// Inner self-request for fibers/test_await_keeps_nothing_after_held_back_bailout.
//
// An await releases the closure of each promise it takes, and releasing it
// here runs a destructor that raises a fatal error. What the await does after
// that release is left behind by the bailout in a worker, whose heap outlives
// the request: a result written into the call's return value, or another
// promise's closure that the await went on to release.

$case = (string) ($_GET['case'] ?? '');

// A compile error, which no error handler sees: the engine bails out on the
// spot. The compile fails before the class is bound, so nothing is declared.
$fatal = static function (): void {
    eval('abstract class OxphpHeldBackBailoutAwaitResult { abstract public function f() {} }');
};

$fatalOnDestruct = static fn (): object => new class ($fatal) {
    public function __construct(private \Closure $fatal)
    {
    }

    public function __destruct()
    {
        ($this->fatal)();
    }
};

switch ($case) {
    case 'await':
    case 'await_race':
        $box = null;
        $p = oxphp_async(static function () use (&$box): \OxPHP\Shared\Map {
            $m = new \OxPHP\Shared\Map();
            $m->set('k', 'v');

            return $m;
        });
        // Assigned after the dispatch, which checks only what the closure
        // captured then; the closure is now the only holder.
        $box = $fatalOnDestruct();
        unset($box);
        $case === 'await' ? oxphp_async_await($p) : oxphp_async_await_race([$p]);
        break;

    case 'await_any_next':
        // The first promise is rejected, so await_any releases its closure
        // and moves on to the second, whose closure holds a Shared\Map.
        $box = null;
        $first = oxphp_async(static function () use (&$box): int {
            throw new \RuntimeException('rejected');
        });
        $map = new \OxPHP\Shared\Map();
        $second = oxphp_async(static function () use ($map): int {
            return 2;
        });
        unset($map);
        $box = $fatalOnDestruct();
        unset($box);
        // Both settle before the await looks, so it takes both in one pass.
        usleep(300_000);
        oxphp_async_await_any([$first, $second]);
        break;

    case 'none':
        echo "OK\n";
        return;

    default:
        echo "FAIL: unknown case\n";
        return;
}

echo "NOT-REACHED\n";
