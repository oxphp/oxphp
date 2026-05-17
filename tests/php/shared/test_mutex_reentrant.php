<?php
header('Content-Type: text/plain');

$m = new OxPHP\Shared\Mutex(0);

$caught = false;
try {
    $m->withLock(function(&$s) use ($m) {
        $m->withLock(function(&$s2) { $s2++; });
        return $s;
    });
} catch (\OxPHP\Shared\DeadlockException $e) {
    $caught = true;
}
if (!$caught) { echo "FAIL: recursive withLock did not throw DeadlockException\n"; exit; }

echo "OK\n";
