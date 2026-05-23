<?php
// getMany yields present keys lazily, skips absent, supports early break.
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();
$m->set('a', 1);
$m->set('b', 2);
$m->set('d', 4);

$out = [];
foreach ($m->getMany(['a', 'b', 'c', 'd']) as $k => $v) {
    $out[$k] = $v;
    if ($k === 'b') {
        break;
    }
}
if ($out !== ['a' => 1, 'b' => 2]) {
    echo "FAIL: lazy/break wrong: " . json_encode($out) . "\n";
    exit;
}

$all = [];
foreach ($m->getMany(['a', 'b', 'c', 'd']) as $k => $v) {
    $all[$k] = $v;
}
if ($all !== ['a' => 1, 'b' => 2, 'd' => 4]) {
    echo "FAIL: absent key not skipped: " . json_encode($all) . "\n";
    exit;
}
echo "OK\n";
