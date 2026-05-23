<?php
// count() is exact when quiescent (single-threaded sequence).
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();
for ($i = 0; $i < 500; $i++) {
    $m->set("k$i", $i);
}
if ($m->count() !== 500) {
    echo "FAIL: count after insert = " . $m->count() . "\n";
    exit;
}
for ($i = 0; $i < 200; $i++) {
    $m->remove("k$i");
}
if ($m->count() !== 300) {
    echo "FAIL: count after remove = " . $m->count() . "\n";
    exit;
}
echo "OK\n";
