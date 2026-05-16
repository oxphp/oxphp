<?php
/**
 * $destroy invocation on pool drop.
 *
 * Verifies that PoolInner::on_drop drains idle deques and calls
 * the user-supplied $destroy callback for each pooled resource
 * exactly once. Also covers the null-$destroy path: dropping a
 * pool whose destroy callback is null must not crash.
 */

header('Content-Type: text/plain');

// ── Scenario 1: destroy is called once per idle resource on drop ──
$destroyed = [];
$pool = new OxPHP\Shared\Pool(
    function (): object {
        return (object) ['id' => uniqid('res_', true)];
    },
    function (object $r) use (&$destroyed): void {
        $destroyed[] = $r->id;
    },
    2, // maxSize — force two distinct slots below
);

// Acquire both slots simultaneously so the second acquire mints a
// new resource (idle reuse would give us only one slot to destroy).
$h1 = $pool->acquire();
$h2 = $pool->acquire();
$id1 = $h1->get()->id;
$id2 = $h2->get()->id;
$pool->release($h1);
$pool->release($h2);

if ($pool->count() !== 2) { echo "FAIL: expected size 2, got " . $pool->count() . "\n"; exit; }
if ($pool->idle() !== 2) { echo "FAIL: expected idle 2, got " . $pool->idle() . "\n"; exit; }
if (count($destroyed) !== 0) { echo "FAIL: destroy must not run before drop\n"; exit; }

// Drop the last ref → registry drops the Pool → on_drop drains idle
// deques → destroy runs for each slot. The $destroyed array is
// captured by reference so we can observe the invocations here.
unset($pool);

if (count($destroyed) !== 2) {
    echo "FAIL: expected 2 destroy invocations, got " . count($destroyed) . "\n";
    exit;
}

$seen = array_flip($destroyed);
if (!isset($seen[$id1])) { echo "FAIL: first resource not destroyed\n"; exit; }
if (!isset($seen[$id2])) { echo "FAIL: second resource not destroyed\n"; exit; }

// ── Scenario 2: null $destroy must not crash on drop ──────────────
$pool2 = new OxPHP\Shared\Pool(
    fn(): object => new stdClass(),
    null, // no destroy callback
    1,
);
$h = $pool2->acquire();
$pool2->release($h);
unset($pool2); // must not crash; slot freed by zval_ptr_dtor alone

echo "OK\n";
