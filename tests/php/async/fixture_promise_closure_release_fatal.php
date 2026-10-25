<?php

declare(strict_types=1);

// Inner self-request for async/test_promise_closure_release_fatal.
//
// Awaiting a promise releases the closure it was dispatched with, and
// releasing the closure releases what it captured by reference: a destructor
// runs inside the await. The task returns a Shared\Map, which the await has
// not yet handed to the script when the destructor raises its fatal error.
//
// A promise never awaited has its closure released at the request's end
// instead, after the request's destructors have been called and its output
// buffers flushed. An object an output handler created during that flush has
// not had its destructor called yet, and releasing the closure runs it.

$run = (string) ($_GET['run'] ?? '');
$case = (string) ($_GET['case'] ?? '');

// A compile error, which no error handler sees: the engine bails out on the
// spot. The compile fails before the class is bound, so nothing is declared.
$fatal = static function (): void {
    eval('abstract class OxphpPromiseClosureFatal { abstract public function f() {} }');
};

switch ($case) {
    case 'await':
        $box = null;
        $p = oxphp_async(static function () use (&$box): \OxPHP\Shared\Map {
            $m = new \OxPHP\Shared\Map();
            $m->set('k', 'v');

            return $m;
        });
        // Assigned after the dispatch, which checks only what the closure
        // captured then; the closure is now the only holder.
        $box = new class ($fatal) {
            public function __construct(private \Closure $fatal)
            {
            }

            public function __destruct()
            {
                ($this->fatal)();
            }
        };
        unset($box);
        oxphp_async_await($p);
        break;

    case 'shutdown':
        // In a function, so that once it returns the promise's closure and
        // the output handler are all that hold $box.
        (static function () use ($fatal, $run): void {
            $box = null;
            oxphp_async(static function () use (&$box): int {
                return 1;
            });
            ob_start(static function (string $buffer) use (&$box, $fatal, $run): string {
                $box = new class ($fatal, $run) {
                    public function __construct(private \Closure $fatal, private string $run)
                    {
                    }

                    public function __destruct()
                    {
                        file_put_contents(sys_get_temp_dir() . "/oxphp-promise-closure-shutdown-{$this->run}", 'ran');
                        ($this->fatal)();
                    }
                };

                return $buffer;
            });
        })();
        echo "DISPATCHED\n";
        return;

    case 'async':
        // Raises nothing, and calls into the server: a bailout left waiting
        // on this thread would be raised here.
        echo 'ASYNC: ', oxphp_async_await(oxphp_async(static fn (): int => 42)), "\n";
        return;

    case 'none':
        echo "OK\n";
        return;

    default:
        echo "FAIL: unknown case\n";
        return;
}

echo "NOT-REACHED\n";
