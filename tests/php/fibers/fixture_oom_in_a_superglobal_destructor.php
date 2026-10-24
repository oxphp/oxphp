<?php

declare(strict_types=1);

// Inner self-request for
// fibers/test_oom_in_a_superglobal_destructor_spares_a_parked_request.
//
// ?step=oom leaves an object in $_GET whose destructor runs out of memory. A
// worker drops a request's superglobals after it has sent the response, with no
// request current, so that is where the destructor runs: between two slices of
// whatever else the worker is serving. It leaves a mark first, so the test can
// tell a destructor that ran from one that never did.
//
// ?step=probe is a request after it: it says it ran, that a callback still runs,
// and so does its shutdown function.

$step = $_GET['step'] ?? 'oom';

if ($step === 'probe') {
    register_shutdown_function(static function (): void {
        echo "SHUTDOWN-RAN\n";
    });
    echo "PROBE-RAN\n";
    echo 'callback: ', json_encode(array_map(static fn (int $n): int => $n * 2, [1, 2])), "\n";
    echo 'worker started: ', sprintf('%.6f', OxPHP\Server\Worker::current()->startTime()), "\n";
    return;
}

$marker = sys_get_temp_dir() . '/oxphp-superglobal-oom-' . preg_replace('/[^0-9a-f]/', '', $_GET['run'] ?? '');

// Anonymous: a named class declared here would be declared again by the next
// request on this worker to include this file.
$_GET['held'] = new class ($marker) {
    public function __construct(private string $marker)
    {
    }

    public function __destruct()
    {
        file_put_contents($this->marker, 'ran');
        $s = 'x';
        for (;;) {
            $s .= $s;
        }
    }
};

echo "OOM-ARMED\n";
