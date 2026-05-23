<?php
// forEach visits entries and stops early when the callback returns false.
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();
for ($i = 0; $i < 200; $i++) {
    $m->set("k$i", $i);
}

$seen = 0;
$m->forEach(function ($k, $v) use (&$seen) {
    $seen++;
    return $seen < 50 ? null : false; // stop after the 50th
});
if ($seen !== 50) {
    echo "FAIL: early stop not honoured, seen=$seen\n";
    exit;
}

// Full traversal visits every key exactly once.
$visited = [];
$m->forEach(function ($k, $v) use (&$visited) {
    $visited[$k] = $v;
});
if (count($visited) !== 200) {
    echo "FAIL: full traversal saw " . count($visited) . " keys\n";
    exit;
}
echo "OK\n";
