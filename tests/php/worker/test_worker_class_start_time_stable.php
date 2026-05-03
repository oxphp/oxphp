<?php
$t1 = OxPHP\Server\Worker::current()->getStartTime();
if (!is_float($t1) || $t1 <= 0.0) {
    http_response_code(500);
    echo "FAIL: getStartTime() returned non-positive float: " . var_export($t1, true) . "\n";
    exit;
}
$now = microtime(true);
if ($t1 > $now) {
    http_response_code(500);
    echo "FAIL: getStartTime() in the future: $t1 > $now\n";
    exit;
}
echo "OK start=$t1 now=$now delta=" . ($now - $t1) . "\n";
