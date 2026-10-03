<?php
/**
 * A class written in PHP cannot extend a Shared type: every class the server
 * registers as OxPHP\Shared\Shareable is final.
 *
 * A subclass inherits the way a Shared object is allocated but has no storage
 * registered under its own name, so the server used to hand it the storage of
 * the first class any plugin registered. In every build that has the Shared
 * types that class is OxPHP\Async\AsyncException, which has no storage: the
 * subclass's constructor failed, and with the constructor overridden the
 * object was turned away wherever a Shared handle is read, only because there
 * was no storage to read one from. The stub has always declared these classes
 * final, and now the engine enforces it: the declaration is refused with the
 * engine's own error, and a test-double generator that checks for final
 * classes refuses them before it declares a subclass.
 *
 * Reported from a shutdown function for the same reasons as
 * test_shareable_userland_implementer_refused.php.
 */
header('Content-Type: text/plain');
ini_set('display_errors', '0');

$reported = false;
$declLine = 0;

register_shutdown_function(static function () use (&$reported, &$declLine): void {
    if ($reported) {
        return;
    }
    $e = error_get_last();
    if ($e === null) {
        echo "FAIL: the request ended without an error\n";
        return;
    }
    if ($e['type'] !== E_COMPILE_ERROR) {
        echo "FAIL: expected E_COMPILE_ERROR, got type {$e['type']}: {$e['message']}\n";
        return;
    }
    $want = 'Class ShareableSubclass cannot extend final class OxPHP\Shared\Counter';
    if ($e['message'] !== $want) {
        echo "FAIL: unexpected message: {$e['message']}\n";
        return;
    }
    if ($e['line'] !== $declLine) {
        echo "FAIL: the fatal was raised on line {$e['line']}, the declaration is on line {$declLine}\n";
        return;
    }
    echo "OK\n";
});

$shareable = 0;
foreach (get_declared_classes() as $class) {
    $rc = new ReflectionClass($class);
    if (!$rc->isInternal() || !$rc->implementsInterface('OxPHP\Shared\Shareable')) {
        continue;
    }
    $shareable++;
    if (!$rc->isFinal()) {
        $reported = true;
        echo "FAIL: {$class} implements OxPHP\\Shared\\Shareable but is not final\n";
        return;
    }
}
if ($shareable === 0) {
    $reported = true;
    echo "FAIL: no class implements OxPHP\\Shared\\Shareable\n";
    return;
}

// Behind a condition the compiler cannot fold: a class whose parent is already
// linked is otherwise bound while the file compiles, before the shutdown
// function above exists.
$declLine = __LINE__ + 2;
if (count($_SERVER) > 0) {
    class ShareableSubclass extends OxPHP\Shared\Counter {}
}

$reported = true;
echo "FAIL: a PHP class extending OxPHP\\Shared\\Counter was accepted\n";
