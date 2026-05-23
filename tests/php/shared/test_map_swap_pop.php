<?php
// swap/pop return the previous value; null when the key was absent.
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();
if ($m->swap('k', 1) !== null) {
    echo "FAIL: swap on absent should return null\n";
    exit;
}
if ($m->swap('k', 2) !== 1) {
    echo "FAIL: swap should return prev 1\n";
    exit;
}
if ($m->pop('k') !== 2) {
    echo "FAIL: pop should return prev 2\n";
    exit;
}
if ($m->pop('k') !== null) {
    echo "FAIL: pop on absent should return null\n";
    exit;
}
echo "OK\n";
