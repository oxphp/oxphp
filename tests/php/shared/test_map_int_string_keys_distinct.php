<?php
// int 123 and string "123" are distinct keys (no PHP key coercion).
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();
$m->set(123, 'int');
$m->set('123', 'str');
if ($m->get(123) !== 'int' || $m->get('123') !== 'str') {
    echo "FAIL: int/string keys collided\n";
    exit;
}
if ($m->count() !== 2) {
    echo "FAIL: count != 2 (got " . $m->count() . ")\n";
    exit;
}
echo "OK\n";
