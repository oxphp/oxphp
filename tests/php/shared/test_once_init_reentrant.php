<?php
header('Content-Type: text/plain');

$o = new OxPHP\Shared\Once();

$caught = false;
try {
    $o->init(function() use ($o) {
        $o->init(fn() => 99);
        return 'unreachable';
    });
} catch (OxPHP\Shared\DeadlockException $e) {
    $caught = true;
}
if (!$caught) {
    echo "FAIL: recursive init did not throw DeadlockException\n"; exit;
}
if ($o->isInitialized()) {
    echo "FAIL: Once should stay uninitialised when outer factory aborted\n"; exit;
}

echo "OK\n";
