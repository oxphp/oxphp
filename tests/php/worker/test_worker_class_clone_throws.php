<?php
$w = OxPHP\Server\Worker::current();
try {
    $clone = clone $w;
} catch (\Error $e) {
    if (str_contains($e->getMessage(), 'Cloning')) {
        echo "OK: " . $e->getMessage() . "\n";
        exit;
    }
    http_response_code(500);
    echo "FAIL: wrong message: " . $e->getMessage() . "\n";
    exit;
}
http_response_code(500);
echo "FAIL: no exception thrown\n";
