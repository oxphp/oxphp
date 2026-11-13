<?php

declare(strict_types=1);

// A request ended by its time limit inside an internal function, on a worker
// whose cycle collector php.ini switched off at startup. The engine raises the
// collector's guard for the whole of a fatal, and the worker lowers it again
// once it has recovered — except here, where the guard was up before the fatal
// too: with the collector off from startup there is no buffer of possible roots
// at all, and the guard is the only thing that keeps the engine from writing
// one into it.
//
// What the guard looks like afterwards is for the probe on the next line, which
// the same worker serves.

require_once __DIR__ . '/gcoff_state.php';

oxphp_gcoff_record_worker();

set_time_limit(1);
array_map(static function (int $i): int {
    $spin = 0;
    while (true) {
        $spin++;
    }
}, [1]);

echo 'NOT-REACHED';
