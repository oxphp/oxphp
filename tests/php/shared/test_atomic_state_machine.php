<?php
/**
 * State machine via Atomic CAS: 0=idle, 1=busy, 2=done.
 * (Migrated from former Counter docs example — Counter is no longer
 * the right primitive for this.)
 */

use OxPHP\Shared\Atomic;

header('Content-Type: text/plain');

$state = new Atomic(initial: 0);

if (!$state->compareAndSet(expect: 0, new: 1)) {
    echo "FAIL: idle → busy transition\n"; exit;
}
if ($state->compareAndSet(expect: 0, new: 1)) {
    echo "FAIL: re-entry should miss\n"; exit;
}
if ($state->load() !== 1) { echo "FAIL: state should be 1 (busy)\n"; exit; }

if (!$state->compareAndSet(expect: 1, new: 2)) {
    echo "FAIL: busy → done transition\n"; exit;
}
if ($state->load() !== 2) { echo "FAIL: state should be 2 (done)\n"; exit; }

echo "OK\n";
