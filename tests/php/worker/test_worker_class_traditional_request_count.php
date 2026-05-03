<?php
$worker = OxPHP\Server\Worker::current();
$count = $worker->getRequestCount();
if (!is_int($count) || $count < 1) {
    http_response_code(500);
    echo "FAIL: getRequestCount() = " . var_export($count, true) . ", expected int >= 1\n";
    exit;
}
echo "OK " . json_encode([
    'worker_id' => $worker->getId(),
    'request_count' => $count,
]) . "\n";
