<?php
/**
 * Channel — unified timeout contract:
 *   null     → wait forever
 *   0.0      → try (immediate)
 *   > 0      → bounded wait
 *   INF      → forever
 *   NaN      → TypeException
 *   negative → TypeException
 */
header('Content-Type: text/plain');

// 0.0 = try → recv on empty returns null without blocking.
$ch = new OxPHP\Shared\Channel(1);
$start = microtime(true);
$got = $ch->recv(0.0);
$elapsed = microtime(true) - $start;
if ($got !== null) { echo "FAIL: recv(0.0) on empty must be null\n"; exit; }
if ($elapsed >= 0.05) { echo "FAIL: recv(0.0) must be immediate, elapsed=$elapsed\n"; exit; }

// 0.0 = try → send on full throws TimeoutException without blocking.
$ch->send('only-slot');
$start = microtime(true);
$threw = false;
try {
    $ch->send('over', 0.0);
} catch (OxPHP\Shared\TimeoutException $e) {
    $threw = true;
}
$elapsed = microtime(true) - $start;
if (!$threw) { echo "FAIL: send(0.0) on full must throw TimeoutException\n"; exit; }
if ($elapsed >= 0.05) { echo "FAIL: send(0.0) must be immediate, elapsed=$elapsed\n"; exit; }

// INF = forever — pre-loaded channel so recv returns immediately.
// This verifies INF is accepted (not rejected as TypeException) and behaves
// like a no-deadline wait when a value is already available.
$ch2 = new OxPHP\Shared\Channel(1);
$ch2->send('payload');
$got = $ch2->recv(INF);
if ($got !== 'payload') { echo "FAIL: recv(INF) on non-empty channel must return value, got=" . var_export($got, true) . "\n"; exit; }

// INF send — channel has free space, must succeed immediately.
$ch2->send('second', INF);
$got = $ch2->recv();
if ($got !== 'second') { echo "FAIL: send(INF) on non-full channel must succeed, got=" . var_export($got, true) . "\n"; exit; }

// NaN → TypeException.
$ch3 = new OxPHP\Shared\Channel(1);
$caught = null;
try {
    $ch3->recv(NAN);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: recv(NaN) must throw TypeException\n"; exit; }

// Negative → TypeException.
$caught = null;
try {
    $ch3->recv(-0.5);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: recv(-0.5) must throw TypeException\n"; exit; }

// Negative send → TypeException.
$caught = null;
try {
    $ch3->send('x', -1.0);
} catch (OxPHP\Shared\TypeException $e) {
    $caught = $e;
}
if ($caught === null) { echo "FAIL: send(-1.0) must throw TypeException\n"; exit; }

echo "OK\n";
