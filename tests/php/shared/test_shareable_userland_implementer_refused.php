<?php
/**
 * A class written in PHP cannot implement OxPHP\Shared\Shareable.
 *
 * The interface marks the objects the server may hand to another thread, and
 * to do that it reads the object's storage from a block laid out in front of
 * the object — a block only the server's own Shared\* classes allocate. A PHP
 * class implementing it used to be accepted, and the first time one of its
 * objects was passed to or returned from another thread, or put into a Shared
 * type — oxphp_async(), a Mutex, a Map or a Channel, for example — the server
 * read whatever memory sat in front of that object as a pointer, which could
 * crash the process. The declaration is now refused instead.
 *
 * Runs in the shared_off profile too: SHARED_ENABLED=false removes the Shared
 * classes but not the interface, and there a PHP class was the only thing that
 * could implement it.
 *
 * A fatal cannot be caught, so the result is reported by a shutdown function,
 * and display_errors is off so the fatal's own text does not reach the body
 * ahead of it. The test stops right after the declaration either way: on a
 * server that accepts it, going on to pass the object anywhere could take the
 * process down with every test after this one.
 */
header('Content-Type: text/plain');
ini_set('display_errors', '0');

$declared = false;
$declLine = 0;

register_shutdown_function(static function () use (&$declared, &$declLine): void {
    if ($declared) {
        return;
    }
    $e = error_get_last();
    if ($e === null) {
        echo "FAIL: the request ended without an error\n";
        return;
    }
    if ($e['type'] !== E_ERROR) {
        echo "FAIL: expected E_ERROR, got type {$e['type']}: {$e['message']}\n";
        return;
    }
    $want = 'Class ShareableImpostor cannot implement interface OxPHP\Shared\Shareable, '
        . 'it is reserved for the OxPHP\Shared types';
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

// A class that implements an interface is never linked while the file compiles,
// so this declaration runs here, after the shutdown function above exists.
$declLine = __LINE__ + 1;
class ShareableImpostor implements OxPHP\Shared\Shareable {}

$declared = true;
echo "FAIL: a PHP class implementing OxPHP\\Shared\\Shareable was accepted\n";
