<?php
// Reentrant getOrInit from within its own factory -> DeadlockException.
header('Content-Type: text/plain');

use OxPHP\Shared\Once;
use OxPHP\Shared\Once\Status;

$o = new Once();

$caught = false;
try {
    $o->getOrInit(function() use ($o) {
        $o->getOrInit(fn() => 99);
        return 'unreachable';
    });
} catch (OxPHP\Shared\DeadlockException $e) {
    $caught = true;
}
if (!$caught) { echo "FAIL: recursive getOrInit did not throw DeadlockException\n"; exit; }
if ($o->status() !== Status::Uninitialized) {
    echo "FAIL: Once should stay uninitialised when outer factory aborted\n"; exit;
}

echo "OK\n";
