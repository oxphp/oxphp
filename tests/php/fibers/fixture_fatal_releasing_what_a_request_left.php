<?php

declare(strict_types=1);

// Inner self-request for fibers/test_fatal_releasing_what_a_request_left_keeps_the_worker
// and fibers/test_fatal_releasing_a_tasks_closure_keeps_other_destructors.
//
// Each step leaves behind something whose release, once the handler has
// returned, runs a destructor that throws or raises a fatal error. The
// destructor writes the marker first, so the test can tell one that ran from
// one that never did.
//
// ?step=unstarted dispatches a task that busies the profile's one async thread
// and, once it is running, a second one that has captured the object by
// reference. The second has not started when the request ends, so the request's
// end releases its closure at once, and the object with it.
//
// ?step=started dispatches one task that has captured the object by reference
// and waits until it runs. A running task is cancelled as the request ends and
// its closure released once it has stopped — after the response has gone out,
// between two slices of whatever the worker serves next.
//
// ?step=superglobal-throw and ?step=superglobal-fatal leave the object in
// $_GET, which the worker empties as the request ends.
//
// ?step=probe is a request after it: it says it ran, that a callback still runs,
// and which worker served it.

$step = (string) ($_GET['step'] ?? '');
$run = preg_replace('/[^0-9a-f]/', '', (string) ($_GET['run'] ?? ''));
$base = sys_get_temp_dir() . '/oxphp-release-left-' . $run;

if ($step === 'probe') {
    echo "PROBE-RAN\n";
    echo 'callback: ', json_encode(array_map(static fn (int $n): int => $n * 2, [1, 2])), "\n";
    echo 'worker started: ', sprintf('%.6f', OxPHP\Server\Worker::current()->startTime()), "\n";
    return;
}

/** Waits for a task to say it runs; usleep() parks this request meanwhile. */
$waitFor = static function (string $file): bool {
    for ($i = 0; $i < 100; $i++) {
        if (is_file($file)) {
            return true;
        }
        usleep(20_000);
    }

    return false;
};

// Anonymous: a named class declared here would be declared again by the next
// request on this worker to include this file. The fatal is a compile error,
// which no error handler sees; the compile fails before the class is bound, so
// nothing is declared.
$held = static fn (string $marker, string $ending): object => new class ($marker, $ending) {
    public function __construct(private string $marker, private string $ending)
    {
    }

    public function __destruct()
    {
        file_put_contents($this->marker, 'ran');
        if ($this->ending === 'throw') {
            throw new \RuntimeException('thrown by a destructor run as the request ends');
        }
        eval('abstract class OxphpReleaseLeftFatal { abstract public function f() {} }');
    }
};

switch ($step) {
    case 'unstarted':
        oxphp_async(static function () use ($base): int {
            file_put_contents("$base.blocker", 'running');
            // On the CPU, not parked: a task that parks hands the thread to
            // the next task, which would then start.
            $end = microtime(true) + 3.0;
            while (microtime(true) < $end) {
            }

            return 1;
        });
        if (!$waitFor("$base.blocker")) {
            echo "BLOCKER-NEVER-RAN\n";
            return;
        }
        $box = null;
        oxphp_async(static function () use (&$box): int {
            return 2;
        });
        // Assigned after the dispatch, which checks only what the closure
        // captured then; the closure is now the only holder.
        $box = $held("$base.ran", 'fatal');
        unset($box);
        echo "DISPATCHED\n";
        return;

    case 'started':
        $box = null;
        oxphp_async(static function () use (&$box, $base): int {
            file_put_contents("$base.task", 'running');
            usleep(300_000);

            return 1;
        });
        if (!$waitFor("$base.task")) {
            echo "TASK-NEVER-RAN\n";
            return;
        }
        $box = $held("$base.ran", 'fatal');
        unset($box);
        echo "DISPATCHED\n";
        return;

    case 'superglobal-throw':
    case 'superglobal-fatal':
        $_GET['held'] = $held("$base.ran", $step === 'superglobal-throw' ? 'throw' : 'fatal');
        echo "LEFT-IN-GET\n";
        return;

    default:
        echo "FAIL: unknown step\n";
        return;
}
