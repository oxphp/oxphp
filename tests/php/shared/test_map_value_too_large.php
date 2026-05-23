<?php
// A value over the per-value cap (default 1 MiB) is rejected.
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();
$big = str_repeat('x', (1 << 20) + 4096); // > 1 MiB
try {
    $m->set('k', $big);
    echo "FAIL: oversize value accepted\n";
    exit;
} catch (OxPHP\Shared\ValueTooLargeException $e) {
    // expected
}
echo "OK\n";
