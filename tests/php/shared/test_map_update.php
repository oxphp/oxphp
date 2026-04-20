<?php
/**
 * Map — atomic RMW via update() / getOrSet() / updateMany().
 */

header('Content-Type: text/plain');

$m = new OxPHP\Shared\Map();

// update on missing key: closure gets null.
$ret = $m->update('counter', function($cur) {
    if ($cur !== null) { throw new RuntimeException("missing key should give null"); }
    return 1;
});
if ($ret !== 1) { echo "FAIL: update missing returned $ret\n"; exit; }
if ($m->get('counter') !== 1) { echo "FAIL: update did not store\n"; exit; }

// update existing: closure gets current, returns new.
$ret = $m->update('counter', function($cur) {
    return $cur + 10;
});
if ($ret !== 11) { echo "FAIL: update existing\n"; exit; }
if ($m->get('counter') !== 11) { echo "FAIL: update store new\n"; exit; }

// update returning null removes the key.
$ret = $m->update('counter', fn($cur) => null);
if ($ret !== null) { echo "FAIL: update null return\n"; exit; }
if ($m->has('counter')) { echo "FAIL: update null did not remove\n"; exit; }

// update returning null on absent key: no-op.
$ret = $m->update('never', fn($cur) => null);
if ($ret !== null) { echo "FAIL: update null absent\n"; exit; }
if ($m->count() !== 0) { echo "FAIL: count after no-op update\n"; exit; }

// update with array value.
$m->set('cfg', ['timeout' => 5, 'retries' => 3]);
$ret = $m->update('cfg', function($cur) {
    $cur['timeout'] = 10;
    return $cur;
});
if ($ret !== ['timeout' => 10, 'retries' => 3]) { echo "FAIL: update array\n"; exit; }
if ($m->get('cfg') !== ['timeout' => 10, 'retries' => 3]) { echo "FAIL: update array stored\n"; exit; }

// getOrSet: hit path — factory must NOT run.
$called = false;
$ret = $m->getOrSet('cfg', function() use (&$called) {
    $called = true;
    return ['never' => 'run'];
});
if ($called) { echo "FAIL: factory ran on hit\n"; exit; }
if ($ret !== ['timeout' => 10, 'retries' => 3]) { echo "FAIL: getOrSet hit\n"; exit; }

// getOrSet: miss path — factory runs and result is stored.
$called = false;
$ret = $m->getOrSet('fresh', function() use (&$called) {
    $called = true;
    return 42;
});
if (!$called) { echo "FAIL: factory must run on miss\n"; exit; }
if ($ret !== 42) { echo "FAIL: getOrSet miss return\n"; exit; }
if ($m->get('fresh') !== 42) { echo "FAIL: getOrSet miss store\n"; exit; }

// updateMany.
$m->clear();
$m->setMany(['a' => 1, 'b' => 2, 'c' => 3]);
$ret = $m->updateMany(['a', 'b', 'c'], fn($cur) => $cur * 10);
if ($ret !== ['a' => 10, 'b' => 20, 'c' => 30]) {
    echo "FAIL: updateMany result " . json_encode($ret) . "\n";
    exit;
}
if ($m->get('a') !== 10 || $m->get('b') !== 20 || $m->get('c') !== 30) {
    echo "FAIL: updateMany not stored\n";
    exit;
}

// updateMany with absent keys: closure sees null.
$m->clear();
$m->set('a', 1);
$ret = $m->updateMany(['a', 'missing'], function($cur) {
    return $cur === null ? 'was_missing' : $cur + 100;
});
$expected = ['a' => 101, 'missing' => 'was_missing'];
if ($ret !== $expected) { echo "FAIL: updateMany with missing " . json_encode($ret) . "\n"; exit; }
if ($m->get('missing') !== 'was_missing') { echo "FAIL: updateMany missing stored\n"; exit; }

// update propagates CycleException.
$a = new OxPHP\Shared\Map();
$threw = false;
try {
    $a->update('self', fn($cur) => $a);
} catch (OxPHP\Shared\CycleException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: update cycle\n"; exit; }

// update closure that throws — original state preserved.
$m->clear();
$m->set('x', 100);
$threw = false;
try {
    $m->update('x', function($cur) { throw new RuntimeException("boom"); });
} catch (RuntimeException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: update closure exception did not propagate\n"; exit; }
if ($m->get('x') !== 100) { echo "FAIL: update damaged state after closure throw\n"; exit; }

echo "OK\n";
