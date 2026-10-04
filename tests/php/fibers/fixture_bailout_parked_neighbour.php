<?php

declare(strict_types=1);

// Neighbour request for the burst tests: does nothing but stay parked, so that
// the worker has all of them in flight at the same time.

usleep(2_000_000);

echo 'parked';
