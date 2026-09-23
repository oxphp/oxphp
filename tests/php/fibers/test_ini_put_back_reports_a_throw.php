<?php

declare(strict_types=1);

// A destructor that runs as a request's ini is put back, and throws, is reported
// as uncaught — as a save handler's throw is, and as it would be anywhere else.
//
// Leaves an object whose destructor throws as the assert callback, the way
// test_ini_put_back_does_not_park leaves one that sleeps: ini_set() enters
// assert.callback in the modified set, assert_options() swaps the callback for
// a closure holding the object without touching that entry, and putting the
// directive back at the end of the request drops the closure. The destructor
// then runs after the request's own code is over, beneath nothing but the
// worker's loop — where the engine neither reports an exception nor passes it
// on. Left there it was dropped the next time the fiber was entered, and the
// request answered 200 as if nothing had happened. The suite line expects the
// 500 an uncaught exception answers; the probe on the next line reads that the
// destructor ran at all, so that 500 is not some other failure's.

require_once __DIR__ . '/ini_put_back_probe.php';

set_error_handler(null);
set_exception_handler(null);

if (!class_exists('OxphpIniPutBackThrower', false)) {
    final class OxphpIniPutBackThrower
    {
        public function __destruct()
        {
            OxphpIniPutBackProbe::$throwerRan = true;
            throw new \RuntimeException('thrown by a destructor the ini put-back ran');
        }
    }
}

OxphpIniPutBackProbe::$throwerRan = false;

// Both calls are deprecated; the deprecations are not what this is about.
@ini_set('assert.callback', 'strlen');
$thrower = new OxphpIniPutBackThrower();
@assert_options(ASSERT_CALLBACK, static function () use ($thrower): void {
});
unset($thrower);

echo 'set';
