<?php
$id = OxPHP\Server\Worker::current()->getId();
if (!is_int($id) || $id < 0) {
    http_response_code(500);
    echo "FAIL: getId() returned non-int or negative: " . var_export($id, true) . "\n";
    exit;
}
echo "OK id=$id\n";
