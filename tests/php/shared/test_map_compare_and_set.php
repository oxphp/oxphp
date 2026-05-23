<?php
// compareAndSet: null is the absence sentinel for insert/remove.
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();
if (!$m->compareAndSet('k', null, 1)) {
    echo "FAIL: insert-if-absent\n";
    exit;
}
if ($m->compareAndSet('k', null, 9)) {
    echo "FAIL: insert when present should fail\n";
    exit;
}
if (!$m->compareAndSet('k', 1, 2)) {
    echo "FAIL: replace on match\n";
    exit;
}
if ($m->compareAndSet('k', 1, 3)) {
    echo "FAIL: replace on mismatch should fail\n";
    exit;
}
if (!$m->compareAndSet('k', 2, null)) {
    echo "FAIL: conditional remove\n";
    exit;
}
if ($m->get('k') !== null) {
    echo "FAIL: key not removed\n";
    exit;
}
echo "OK\n";
