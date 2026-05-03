<?php
$w = OxPHP\Server\Worker::current();

if (!$w->isWorkerMode()) {
    echo "OK (traditional mode — skipping worker-only checks)\n";
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
$count = $w->getRequestCount();
if ($count < 1) {
    http_response_code(500);
    echo "FAIL: getRequestCount() = $count, expected >= 1\n";
    exit;
}

echo "OK count=$count\n";
