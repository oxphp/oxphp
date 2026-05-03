<?php
$w = OxPHP\Server\Worker::current();

if ($w->isWorkerMode()) {
    echo "OK (worker mode — skipping traditional-only checks)\n";
    exit;
}

if ($w->getId() < 0) {
    http_response_code(500);
    echo "FAIL: getId() negative\n";
    exit;
}
if ($w->getStartTime() <= 0.0) {
    http_response_code(500);
    echo "FAIL: getStartTime() non-positive\n";
    exit;
}
if ($w->getRequestCount() < 1) {
    http_response_code(500);
    echo "FAIL: getRequestCount() < 1\n";
    exit;
}

try {
    $w->serve(fn() => null);
    http_response_code(500);
    echo "FAIL: serve() did not throw in traditional\n";
    exit;
} catch (OxPHP\Server\InvalidServeContextException $e) {
    /* expected */
}

echo "OK\n";
