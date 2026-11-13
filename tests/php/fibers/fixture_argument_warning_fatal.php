<?php

declare(strict_types=1);

// A fatal raised from a warning the engine gives while it passes an argument,
// before it has stored it: about an undefined variable (?send=undefined), or
// about a value unpacked by name into a by-reference parameter, from an array
// (?send=array) or a Traversable (?send=unpack). The argument stored ahead of
// it is the probe's $kept object, which the call does own. The slot of the
// warned one still holds what the stack held there: the $held object the
// previous call at the same place was passed, a reference it has already
// given back.
//
// ?send=traced warns about an undefined variable passed to a call nested in
// the arguments of another, in a loop the tracing JIT has compiled by the time
// the warning fatals. The compiled loop links the inner call to the frame only
// when it makes it, so the frame names the outer one as the call it was
// passing arguments to. The outer call's second slot holds the $held object
// the inner call returned on the previous turn, which the outer call has
// already given back.
//
// ?send=autoload fatals in the autoloader of a class instantiated as the second
// argument, from the opcode that begins the constructor call: the call taking
// $kept is open, the constructor call is not.
// See test_fatal_in_an_argument_warning_leaves_its_slot_alone.php.

require_once __DIR__ . '/../engine_fatal.php';
require_once __DIR__ . '/argument_probe.php';

OxphpArgumentProbe::$held ??= new stdClass();
OxphpArgumentProbe::$kept ??= new stdClass();

$send = (string) ($_GET['send'] ?? 'undefined');

// Warnings let through before the one that fatals: enough turns of the loop
// for the JIT to compile it and run it compiled.
$quiet = $send === 'traced' ? 1000 : 0;

set_error_handler(static function () use (&$quiet): bool {
    if ($quiet-- > 0) {
        return true;
    }
    engine_fatal('OxphpArgumentWarningFatal');
}, E_WARNING);
spl_autoload_register('oxphp_argument_autoload');

(static function (string $send): void {
    $k = OxphpArgumentProbe::$kept;
    // In a variable, not a temporary: the operand of the operation that fatals
    // is not released, and an array holding $kept there would keep it counted.
    $args = [$k, 'a' => 1];
    $it = new ArrayIterator(['a' => 1]);
    $o = OxphpArgumentProbe::$held;
    OxphpArgumentProbe::take2($k, $o);
    match ($send) {
        'array' => call_user_func_array([OxphpArgumentProbe::class, 'takeRef2'], $args),
        'unpack' => OxphpArgumentProbe::takeRef2($k, ...$it),
        'traced' => (static function (): void {
            for ($i = 0; $i < 2000; $i++) {
                oxphp_argument_outer(1, oxphp_argument_inner(2, 3, $undefined));
            }
        })(),
        'autoload' => OxphpArgumentProbe::take2($k, new OxphpArgumentUnloaded()),
        default => OxphpArgumentProbe::take2($k, $undefined),
    };
})($send);

echo "NOT-REACHED\n";
