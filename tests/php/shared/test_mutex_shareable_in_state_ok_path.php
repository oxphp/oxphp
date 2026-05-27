<?php
/**
 * Diagnostic: does the OK path of Mutex::withLock also drop embedded
 * Shareables, or only BAD_RETURN? If this test FAILS, the bug is
 * universal across all paths (OK / PHP_THREW / BAD_RETURN), not just
 * BAD_RETURN. If this test PASSES, the bug is BAD_RETURN-specific and
 * I need to understand why my retain fix only works for OK.
 */
header('Content-Type: text/plain');

use OxPHP\Shared\Mutex;
use OxPHP\Shared\Map;

$m = new Mutex(['marker' => 'initial']);

// Good return — exercises the OK path of byref_1_portbuf.
$ret = $m->withLock(function (array &$s) {
    $s = ['inner' => new Map()];
    $s['inner']->set('marker', 'ok-path-survived');
    return true;
});

if ($ret !== true) {
    echo "FAIL: unexpected closure return: ", var_export($ret, true), "\n"; exit;
}

$has_inner = $m->withLock(fn(array &$s) =>
    isset($s['inner']) && is_object($s['inner']) && $s['inner'] instanceof Map
);
if (!$has_inner) {
    $observed = $m->withLock(fn(array &$s) =>
        isset($s['inner']) ? gettype($s['inner']) : ('keys=' . implode(',', array_keys($s)))
    );
    echo "FAIL: OK-path state mutation lost — observed $observed\n"; exit;
}

$marker = $m->withLock(fn(array &$s) => $s['inner']->get('marker'));
if ($marker !== 'ok-path-survived') {
    echo "FAIL: nested Map's marker lost — got ", var_export($marker, true), "\n"; exit;
}

echo "OK\n";
