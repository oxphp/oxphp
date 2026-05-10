<?php
/**
 * Atomic ordering validation — verify InvalidOrderingException for
 * memory-ordering combinations that std::sync::atomic forbids.
 */

use OxPHP\Shared\Atomic;
use OxPHP\Shared\Ordering;
use OxPHP\Shared\InvalidOrderingException;

header('Content-Type: text/plain');

$a = new Atomic();

// load() with Release or AcqRel must throw.
foreach ([Ordering::Release, Ordering::AcqRel] as $bad) {
    $threw = false;
    try {
        $a->load($bad);
    } catch (InvalidOrderingException $e) {
        $threw = true;
    }
    if (!$threw) { echo "FAIL: load({$bad->name}) should throw\n"; exit; }
}

// store() with Acquire or AcqRel must throw.
foreach ([Ordering::Acquire, Ordering::AcqRel] as $bad) {
    $threw = false;
    try {
        $a->store(0, $bad);
    } catch (InvalidOrderingException $e) {
        $threw = true;
    }
    if (!$threw) { echo "FAIL: store({$bad->name}) should throw\n"; exit; }
}

// compareAndSet failure ordering Release/AcqRel must throw.
foreach ([Ordering::Release, Ordering::AcqRel] as $bad) {
    $threw = false;
    try {
        $a->compareAndSet(0, 1, Ordering::SeqCst, $bad);
    } catch (InvalidOrderingException $e) {
        $threw = true;
    }
    if (!$threw) { echo "FAIL: cas failure={$bad->name} should throw\n"; exit; }
}

// All-valid combos must not throw.
$a->load(Ordering::Acquire);
$a->load(Ordering::Relaxed);
$a->load(Ordering::SeqCst);
$a->store(0, Ordering::Release);
$a->store(0, Ordering::Relaxed);
$a->store(0, Ordering::SeqCst);
$a->compareAndSet(0, 1, Ordering::SeqCst, Ordering::Acquire);
$a->compareAndSet(1, 2, Ordering::AcqRel, Ordering::Relaxed);

echo "OK\n";
