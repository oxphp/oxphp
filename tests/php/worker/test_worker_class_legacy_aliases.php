<?php
$w = OxPHP\Server\Worker::current();

if (oxphp_is_worker() !== $w->isWorkerMode()) {
    http_response_code(500);
    echo "FAIL: oxphp_is_worker() !== Worker::isWorkerMode()\n";
    exit;
}

if (oxphp_worker_id() !== $w->id()) {
    http_response_code(500);
    echo "FAIL: oxphp_worker_id() !== Worker::id()\n";
    exit;
}

echo "OK\n";
