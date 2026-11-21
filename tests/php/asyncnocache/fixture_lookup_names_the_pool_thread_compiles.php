<?php

declare(strict_types=1);

// Included by a task of test_task_class_lookup_slot_after_the_pool_thread_compiled_names,
// so the async thread compiles it. Each parameter type spells SplStack its own
// way, so each is a name of its own with a class-lookup slot that thread numbers
// itself, from the start a request numbers the names of the fixtures from; the
// `new` of each fills its slot with SplStack.
$make = static function (
    ?splStack $a = null,
    ?splstack $b = null,
    ?SPLSTACK $c = null,
    ?Splstack $d = null,
    ?sPlStack $e = null,
    ?spLStack $f = null,
    ?splsTack $g = null,
    ?splstAck $h = null,
): array {
    return [
        new splStack(),
        new splstack(),
        new SPLSTACK(),
        new Splstack(),
        new sPlStack(),
        new spLStack(),
        new splsTack(),
        new splstAck(),
    ];
};

return array_map(get_class(...), $make());
