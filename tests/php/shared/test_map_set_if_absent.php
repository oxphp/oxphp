<?php
// setIfAbsent returns null on insert, the existing value otherwise.
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();
if ($m->setIfAbsent('k', 1) !== null) {
    echo "FAIL: insert should return null\n";
    exit;
}
if ($m->setIfAbsent('k', 2) !== 1) {
    echo "FAIL: should return prev value 1\n";
    exit;
}
if ($m->get('k') !== 1) {
    echo "FAIL: value was clobbered\n";
    exit;
}
echo "OK\n";
