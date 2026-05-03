<?php
/* test: Worker::current() returns the same instance per thread.
 * Worker::isWorkerMode() returns the expected boolean for the active mode. */

$a = OxPHP\Server\Worker::current();
$b = OxPHP\Server\Worker::current();

if ($a !== $b) {
    http_response_code(500);
    echo "FAIL: current() returned different instances\n";
    exit;
}
if (!($a instanceof OxPHP\Server\Worker)) {
    http_response_code(500);
    echo "FAIL: current() did not return a Worker instance\n";
    exit;
}

$mode = OxPHP\Server\Worker::isWorkerMode();
if (!is_bool($mode)) {
    http_response_code(500);
    echo "FAIL: isWorkerMode() did not return bool\n";
    exit;
}

echo "OK\n";
