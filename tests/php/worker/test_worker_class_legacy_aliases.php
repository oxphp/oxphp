<?php
$w = OxPHP\Server\Worker::current();

if (oxphp_is_worker() !== $w->isWorkerMode()) {
    http_response_code(500);
    echo "FAIL: oxphp_is_worker() !== Worker::isWorkerMode()\n";
    exit;
}

if (oxphp_worker_id() !== $w->getId()) {
    http_response_code(500);
    echo "FAIL: oxphp_worker_id() !== Worker::getId()\n";
    exit;
}

echo "OK\n";
