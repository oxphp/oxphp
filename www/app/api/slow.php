<?php

$ms = min((int)($_GET['ms'] ?? 1000), 30000);
usleep($ms * 1000);
json_response(200, [
    'slept_ms' => $ms,
    'note'     => 'Use REQUEST_TIMEOUT_SECS to test 504 Gateway Timeout.',
]);
