<?php
/**
 * Multiple independent Pool instances coexist.
 *
 * Verifies:
 *  - Each Pool gets a unique id() from the SharedRegistry.
 *  - Acquire/release on one pool does not affect the other's budget,
 *    idle count, or destroy accounting.
 *  - Dropping pool A runs destroy ONLY for A's resources.
 */

header('Content-Type: text/plain');

$destroyedA = [];
$destroyedB = [];

$poolA = new OxPHP\Shared\Pool(
    fn(): object => (object) ['tag' => 'A-' . uniqid()],
    function (object $r) use (&$destroyedA): void { $destroyedA[] = $r->tag; },
    2,
);

$poolB = new OxPHP\Shared\Pool(
    fn(): object => (object) ['tag' => 'B-' . uniqid()],
    function (object $r) use (&$destroyedB): void { $destroyedB[] = $r->tag; },
    2,
);

if ($poolA->id() === $poolB->id()) { echo "FAIL: pool ids must differ\n"; exit; }

// Populate one slot on each pool (same-thread reuse means successive
// acquire+release keeps the slot count at 1).
$ha = $poolA->acquire();
$aTag = $ha->get()->tag;
$poolA->release($ha);

$hb = $poolB->acquire();
$bTag = $hb->get()->tag;
$poolB->release($hb);

if ($poolA->size() !== 1 || $poolA->idle() !== 1) {
    echo "FAIL: poolA bookkeeping: size=" . $poolA->size() . " idle=" . $poolA->idle() . "\n"; exit;
}
if ($poolB->size() !== 1 || $poolB->idle() !== 1) {
    echo "FAIL: poolB bookkeeping: size=" . $poolB->size() . " idle=" . $poolB->idle() . "\n"; exit;
}

// Drop A only. destroy must fire for A's resource; B untouched.
unset($poolA);

if (count($destroyedA) !== 1)       { echo "FAIL: A destroy count wrong: " . count($destroyedA) . "\n"; exit; }
if ($destroyedA[0] !== $aTag)       { echo "FAIL: A destroyed wrong tag\n"; exit; }
if (count($destroyedB) !== 0)       { echo "FAIL: B destroy must not fire from A drop\n"; exit; }
if ($poolB->size() !== 1)           { echo "FAIL: poolB size must stay 1\n"; exit; }

// Now drop B.
unset($poolB);

if (count($destroyedB) !== 1)       { echo "FAIL: B destroy count wrong: " . count($destroyedB) . "\n"; exit; }
if ($destroyedB[0] !== $bTag)       { echo "FAIL: B destroyed wrong tag\n"; exit; }

echo "OK\n";
