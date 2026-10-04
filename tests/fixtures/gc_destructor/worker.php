<?php
// Worker-mode fixture for tests/worker_gc_destructor.sh. /garbage leaves a
// cycle whose destructor misbehaves in one of four ways, /leak ends a request
// inside usort() so that the worker runs the cycle collector afterwards, and
// everything that is logged after oxphp_worker() returns says what state the
// worker was left in.

// A destructor that throws an exception whose own destructor throws again.
class ChainException extends Exception
{
    public function __destruct()
    {
        error_log('chain-exception-destructed');
        throw new RuntimeException('thrown by the exception\'s destructor');
    }
}

final class ChainCycle
{
    public $self;

    public function __destruct()
    {
        error_log('chain-destructed');
        throw new ChainException('thrown by the cycle\'s destructor');
    }
}

// A destructor that throws an exception whose own destructor ends in a fatal,
// inside the function an observer wraps below.
class FatalException extends Exception
{
    public function __destruct()
    {
        error_log('fatal-exception-destructed');
        watched_fatal();
    }
}

final class FatalChainCycle
{
    public $self;

    public function __destruct()
    {
        error_log('fatal-chain-destructed');
        throw new FatalException('thrown by the cycle\'s destructor');
    }
}

// A destructor that recurses until the memory limit ends it, so the VM stack
// has grown past its first page when the fatal comes.
function recurse(int $depth): int
{
    return recurse($depth + 1) + 1;
}

final class DeepCycle
{
    public $self;

    public function __destruct()
    {
        error_log('deep-destructed');
        recurse(0);
    }
}

// A function an observer wraps, and a destructor that ends in a fatal inside it,
// so the observer's end handler is still pending when the fatal comes.
#[\Attribute(\Attribute::TARGET_FUNCTION)]
class Watched implements \OxPHP\Decorator\AttributeInterface
{
    public function before(\OxPHP\Decorator\Context $ctx): void
    {
    }

    public function after(\OxPHP\Decorator\Context $ctx): void
    {
        error_log('watched-after-ran');
    }
}
oxphp_register_decorator(Watched::class);

#[Watched]
function watched_fatal(): void
{
    error_log('watched-entered');
    str_repeat('x', 512 * 1024 * 1024);
}

final class ObservedCycle
{
    public $self;

    public function __destruct()
    {
        error_log('observed-destructed');
        watched_fatal();
    }
}

function post_loop_call(int $depth): int
{
    $local = [$depth, $depth + 1, $depth + 2];

    return $depth > 0 ? post_loop_call($depth - 1) + count($local) : 0;
}

oxphp_worker(function () {
    $path = parse_url($_SERVER['REQUEST_URI'] ?? '/', PHP_URL_PATH);
    switch ($path) {
        case '/garbage':
            // Refcount 2 -> 1: a possible root, in the collector's buffer and
            // not collected until it next runs.
            $cycle = match ($_GET['kind'] ?? '') {
                'chain' => new ChainCycle(),
                'fatalchain' => new FatalChainCycle(),
                'deep' => new DeepCycle(),
                'observed' => new ObservedCycle(),
            };
            $cycle->self = $cycle;
            $cycle = null;
            echo 'garbage';
            return;

        case '/leak':
            // Parked while /garbage runs and ends, so that the request ended
            // inside usort() below is the one the collector runs after.
            oxphp_sleep(2.0);
            $rows = [];
            for ($i = 0, $n = (int) ($_GET['rows'] ?? 200); $i < $n; $i++) {
                $rows[] = str_pad((string) $i, 250, 'x');
            }
            $calls = 0;
            set_time_limit(1);
            usort($rows, static function (string $a, string $b) use (&$calls): int {
                if (++$calls > 100) {
                    while (true) {
                    }
                }

                return $a <=> $b;
            });
            echo 'leak-finished';
            return;

        default:
            echo 'ok';
            return;
    }
});

// What the stub promises runs on every exit. A call a few thousand frames deep
// is what finds a VM stack whose cursors were left on different pages.
error_log('after-loop-ran');
error_log('after-loop-depth=' . post_loop_call(4000));
error_log('after-loop-done');
