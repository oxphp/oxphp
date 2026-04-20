<?php
/**
 * Channel — tryRecv semantics:
 *   - empty + open → null
 *   - item present → return it
 *   - closed + empty → ClosedException (stricter than recv which returns null)
 */

header('Content-Type: text/plain');

$ch = new OxPHP\Shared\Channel(4);

if ($ch->tryRecv() !== null) { echo "FAIL: empty+open tryRecv should return null\n"; exit; }

$ch->send('x');
if ($ch->tryRecv() !== 'x') { echo "FAIL: tryRecv should return 'x'\n"; exit; }
if ($ch->tryRecv() !== null) { echo "FAIL: drained tryRecv should return null\n"; exit; }

$ch->close();
$threw = false;
try {
    $ch->tryRecv();
} catch (OxPHP\Shared\ClosedException $e) {
    $threw = true;
}
if (!$threw) { echo "FAIL: tryRecv on closed+empty should throw ClosedException\n"; exit; }

echo "OK\n";
