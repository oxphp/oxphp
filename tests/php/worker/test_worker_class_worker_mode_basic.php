<?php
$w = OxPHP\Server\Worker::current();

if (!$w->isWorkerMode()) {
    echo "OK (traditional mode — skipping worker-only checks)\n";
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
$count = $w->requestCount();
if ($count < 1) {
    http_response_code(500);
    echo "FAIL: requestCount() = $count, expected >= 1\n";
    exit;
}

echo "OK count=$count\n";
