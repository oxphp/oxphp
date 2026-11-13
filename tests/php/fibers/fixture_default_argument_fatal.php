<?php

declare(strict_types=1);

// A fatal raised while the engine evaluates the default of a parameter a call
// skipped by naming a later one: the default constructs an object, and its
// constructor fatals. The call has not been entered, but the engine evaluates
// the default with the call standing as the current frame, so the frames the
// fatal leaves behind include one whose arguments are all that is set up. The
// call is made from the calling frame, or through zend_call_function() with
// ?via=reflection.
// See test_fatal_in_a_default_argument_leaves_the_call_alone.php.

require_once __DIR__ . '/../engine_fatal.php';
require_once __DIR__ . '/argument_probe.php';

OxphpArgumentProbe::$held ??= new stdClass();

if (!class_exists('OxphpDefaultArgumentDep', false)) {
    final class OxphpDefaultArgumentDep
    {
        public function __construct()
        {
            engine_fatal('OxphpDefaultArgumentFatal');
        }
    }

    // A variable past the arguments, which a call never entered has not set.
    function oxphp_default_argument_take(object $dep = new OxphpDefaultArgumentDep(), ?object $o = null): void
    {
        $kept = [$dep, $o];
        OxphpArgumentProbe::take($kept);
    }

    // Leaves the object in every slot of the stack its variables stood on,
    // references already given back, where the call made next stands.
    function oxphp_default_argument_spread(object $o): void
    {
        $a = $b = $c = $d = $e = $f = $g = $h = $i = $j = $k = $l = $m = $n = $p = $o;
        OxphpArgumentProbe::take([$a, $b, $c, $d, $e, $f, $g, $h, $i, $j, $k, $l, $m, $n, $p]);
    }
}

(static function (bool $reflection): void {
    $o = OxphpArgumentProbe::$held;
    oxphp_default_argument_spread($o);
    if ($reflection) {
        (new ReflectionFunction('oxphp_default_argument_take'))->invokeArgs(['o' => $o]);
    } else {
        oxphp_default_argument_take(o: $o);
    }
})(($_GET['via'] ?? '') === 'reflection');

echo "NOT-REACHED\n";
