<?php
/**
 * Mutex — unified timeout contract on with():
 *   null=forever, 0.0=try, positive=bounded, INF=forever,
 *   NaN→TypeException, negative→TypeException.
 *
 * Contention semantics (e.g. with(0.0) on a held mutex throws
 * TimeoutException) require a second execution context and are
 * covered indirectly by test_mutex_timeout.php under the async
 * profile. Here we exercise the input-validation matrix and the
 * uncontended success paths, all of which are observable without
 * spawning a fiber.
 */
header('Content-Type: text/plain');

$m = new OxPHP\Shared\Mutex(0);

// null = forever — bare with($fn) succeeds and returns body's value.
$got = $m->with(fn(&$s) => 42);
if ($got !== 42) { echo "FAIL: with(\$fn) (null) must return body value, got " . var_export($got, true) . "\n"; exit; }

// 0.0 = try — uncontended acquisition succeeds.
$got = $m->with(fn(&$s) => 'try-ok', 0.0);
if ($got !== 'try-ok') { echo "FAIL: with(\$fn, 0.0) uncontended must succeed, got " . var_export($got, true) . "\n"; exit; }

// positive = bounded — uncontended acquisition succeeds within budget.
$got = $m->with(fn(&$s) => 'bounded-ok', 1.0);
if ($got !== 'bounded-ok') { echo "FAIL: with(\$fn, 1.0) uncontended must succeed, got " . var_export($got, true) . "\n"; exit; }

// INF = forever — uncontended acquisition succeeds.
$got = $m->with(fn(&$s) => 'inf-ok', INF);
if ($got !== 'inf-ok') { echo "FAIL: with(\$fn, INF) uncontended must succeed, got " . var_export($got, true) . "\n"; exit; }

// NaN → TypeException.
$caught = null;
try {
    $m->with(fn(&$s) => 1, NAN);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: with(\$fn, NaN) must throw TypeException\n"; exit; }

// Negative → TypeException.
$caught = null;
try {
    $m->with(fn(&$s) => 1, -0.5);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: with(\$fn, -0.5) must throw TypeException\n"; exit; }

// State persistence — bare with mutates the stored value.
$m->with(function (&$s) { $s = 7; });
$snapshot = $m->with(fn(&$s) => $s);
if ($snapshot !== 7) { echo "FAIL: stored mutation lost, got " . var_export($snapshot, true) . "\n"; exit; }

echo "OK\n";
