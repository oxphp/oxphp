<?php
// Absent key (or removed key) reads back as null — the absence sentinel.
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();
if ($m->get('missing') !== null) {
    echo "FAIL: absent key not null\n";
    exit;
}
$m->set('k', 7);
$m->remove('k');
if ($m->get('k') !== null) {
    echo "FAIL: removed key not null\n";
    exit;
}
echo "OK\n";
