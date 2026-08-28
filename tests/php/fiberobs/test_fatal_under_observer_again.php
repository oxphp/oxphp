<?php

declare(strict_types=1);

// The second fatal, and the one that matters: it abandons frames at the very
// addresses the chain left by the first is still naming. Writing the stale head
// into a frame that the stale head already leads to closes the chain into a
// loop — a chain with no end, in a place only the engine walks.
//
// Nothing here can see that. The walk happens when a fiber dies, and what a
// request can say is only that the worker is still answering, which the test
// after this one does.
//
// No assertions and no test JSON: the suite line checks the 500 this produces.

require __DIR__ . '/fixture_exhaust_memory.php';
