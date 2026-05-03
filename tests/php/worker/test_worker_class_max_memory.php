<?php
$cap = OxPHP\Server\Worker::current()->getMaxMemoryBytes();
if (!is_int($cap) || $cap < 0) {
    http_response_code(500);
    echo "FAIL: getMaxMemoryBytes() = " . var_export($cap, true) . "\n";
    exit;
}
/* Without WORKER_MAX_MEMORY_MIB, expect 0. With env=64, expect 64*1024*1024. */
echo "OK cap=$cap\n";
