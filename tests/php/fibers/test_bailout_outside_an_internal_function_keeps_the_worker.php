<?php

declare(strict_types=1);

// A request ended in plain PHP code leaves the worker alone however much the
// worker holds afterwards.
//
// What a worker keeps on purpose — a cache, a connection pool, a compiled
// container — is state the application asked for, not a leak, and growing it by
// more than a recycle's threshold in the same request that is then ended must not
// be mistaken for one. Only a request ended with an internal function's call on
// its stack can have had something left behind that nothing frees, so only that
// one is looked at.

require_once __DIR__ . '/bailout_leak_probe.php';

OxphpBailoutLeak::arm('loop');
OxphpBailoutLeak::$retained = OxphpBailoutLeak::rows(OxphpBailoutLeak::BIG_ROWS);

set_time_limit(1);
$spin = 0;
while (true) {
    $spin++;
}
