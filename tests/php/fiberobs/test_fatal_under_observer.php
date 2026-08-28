<?php

declare(strict_types=1);

// The first fatal. Leaves the engine's chain of open calls naming the frames
// it abandoned; the recovery that follows it frees those frames and puts the
// VM stack back where this request started, so the next request lays its own
// frames over the addresses the chain is still naming.
//
// No assertions and no test JSON: the suite line checks the 500 this produces.

require __DIR__ . '/fixture_exhaust_memory.php';
