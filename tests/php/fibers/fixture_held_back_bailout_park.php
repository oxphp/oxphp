<?php

declare(strict_types=1);

// Inner self-request for fibers/test_await_all_no_park_after_held_back_bailout.
//
// An await over several promises releases the closure of each promise it
// takes, and releasing it here runs a destructor that raises a fatal error,
// with another promise still to wait for. The request must end there, not
// park its fiber on that promise: while it is parked, the worker runs the
// other requests it multiplexes, and the fatal must not land in one of them.

$case = (string) ($_GET['case'] ?? '');

// A compile error, which no error handler sees: the engine bails out on the
// spot. The compile fails before the class is bound, so nothing is declared.
$fatal = static function (): void {
    eval('abstract class OxphpHeldBackBailoutPark { abstract public function f() {} }');
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

$slow = static function (): int {
    usleep(800_000);

    return 2;
};

switch ($case) {
    case 'await_all':
        $box = null;
        $first = oxphp_async(static function () use (&$box): int {
            return 1;
        });
        $second = oxphp_async($slow);
        // Assigned after the dispatch, which checks only what the closure
        // captured then; the closure is now the only holder.
        $box = $fatalOnDestruct();
        unset($box);
        oxphp_async_await_all([$first, $second]);
        break;

    case 'await_any':
        // The first promise is rejected, so await_any releases its closure
        // and goes on waiting for the second.
        $box = null;
        $first = oxphp_async(static function () use (&$box): int {
            throw new \RuntimeException('rejected');
        });
        $second = oxphp_async($slow);
        $box = $fatalOnDestruct();
        unset($box);
        oxphp_async_await_any([$first, $second]);
        break;

    case 'spin':
        // Enters a plugin function every few milliseconds for longer than the
        // other case's slow promise takes.
        $until = microtime(true) + 1.5;
        while (microtime(true) < $until) {
            oxphp_async_await(oxphp_async(static fn (): int => 1));
            usleep(10_000);
        }
        echo "SPUN\n";
        return;

    case 'none':
        echo "OK\n";
        return;

    default:
        echo "FAIL: unknown case\n";
        return;
}

echo "NOT-REACHED\n";
