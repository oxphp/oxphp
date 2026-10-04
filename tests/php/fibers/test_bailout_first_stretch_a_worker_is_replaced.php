<?php

declare(strict_types=1);

// Gives the request after it a worker of its own.
//
// The first stretch a worker is busy for is not looked at when a request ended
// inside an internal function is judged, so a test of that has to be the first
// thing a new worker is asked to do. This retires the worker it runs on once it
// has answered, and the pool's replacement is what the next request lands on.

require_once __DIR__ . '/bailout_leak_probe.php';

OxphpBailoutLeak::arm('replaced');
OxPHP\Server\Worker::current()->scheduleExit();

echo 'handed over';
