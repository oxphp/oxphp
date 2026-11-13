<?php

declare(strict_types=1);

require_once __DIR__ . '/after_loop_probe.php';

// A request that asks for the worker's exit and leaves an object in $_GET whose
// destructor runs out of memory. Its end drops $_GET, so the destructor runs as
// the request ends and its error is the request's own: the suite line expects
// the 500 that answers. The worker then leaves its loop, and the entry script's
// code after the oxphp_worker() call runs with nothing of that error standing:
// the engine, left noting it had reported running out of memory, refuses every
// call through zend_call_function() — a callback, a shutdown function, an
// output handler. fibers/..._probe, on the next line, reads what that code found.

$base = sys_get_temp_dir() . '/oxphp-oom-before-exit';
if (is_file("$base.ran")) {
    unlink("$base.ran");
}
OxphpAfterLoop::arm();

OxPHP\Server\Worker::current()->scheduleExit();

// Anonymous: a named class declared here would be declared again by the next
// request on this worker to include this file.
$_GET['held'] = new class ("$base.ran") {
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
