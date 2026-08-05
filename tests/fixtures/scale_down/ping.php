<?php
// Traditional-mode fixture for tests/worker_scale_down.sh: the cheapest
// request that still counts as an arrival for the pool's idle stamp.
header('Content-Type: text/plain');
echo 'pong ', oxphp_worker_id(), "\n";
