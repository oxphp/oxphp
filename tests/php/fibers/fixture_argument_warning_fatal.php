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
// See test_fatal_in_an_argument_warning_leaves_its_slot_alone.php.

require_once __DIR__ . '/../engine_fatal.php';
require_once __DIR__ . '/argument_probe.php';

OxphpArgumentProbe::$held ??= new stdClass();
OxphpArgumentProbe::$kept ??= new stdClass();

set_error_handler(static function (): bool {
    engine_fatal('OxphpArgumentWarningFatal');
}, E_WARNING);

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
        default => OxphpArgumentProbe::take2($k, $undefined),
    };
})((string) ($_GET['send'] ?? 'undefined'));

echo "NOT-REACHED\n";
