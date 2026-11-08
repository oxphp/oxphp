<?php

declare(strict_types=1);

// One request of the load in test_worker_load_then_reset: it parks, so that a
// worker multiplexes many of them and always has one in flight, for a time of
// its own, so that no moment finds them all ended.

header('Content-Type: application/json');
oxphp_sleep(mt_rand(100, 300) / 1000);
echo '{}';
