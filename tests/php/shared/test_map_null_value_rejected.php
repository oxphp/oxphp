<?php
// null is forbidden as a stored value across every write path.
header('Content-Type: text/plain');
$m = new OxPHP\Shared\Map();
foreach (['set', 'setIfAbsent', 'swap'] as $method) {
    try {
        $m->$method('k', null);
        echo "FAIL: $method(null) did not throw\n";
        exit;
    } catch (OxPHP\Shared\TypeException $e) {
        // expected
    }
}
echo "OK\n";
