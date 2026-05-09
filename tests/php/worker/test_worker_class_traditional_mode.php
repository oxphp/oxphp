<?php
$w = OxPHP\Server\Worker::current();

if ($w->isWorkerMode()) {
    echo "OK (worker mode — skipping traditional-only checks)\n";
    exit;
}

if ($w->id() < 0) {
    http_response_code(500);
    echo "FAIL: id() negative\n";
    exit;
}
if ($w->startTime() <= 0.0) {
    http_response_code(500);
    echo "FAIL: startTime() non-positive\n";
    exit;
}
if ($w->requestCount() < 1) {
    http_response_code(500);
    echo "FAIL: requestCount() < 1\n";
    exit;
}

try {
    $w->serve(fn() => null);
    http_response_code(500);
    echo "FAIL: serve() did not throw in traditional\n";
    exit;
} catch (OxPHP\Server\Exception\InvalidServeContextException $e) {
    /* expected */
}

echo "OK\n";
