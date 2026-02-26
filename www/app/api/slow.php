<?php

$ms = min((int)($_GET['ms'] ?? 1000), 30000);

// ?finish=1 — test oxphp_finish_request(): send response, then sleep in background
if (!empty($_GET['finish'])) {
    json_response(200, [
        'finished'  => true,
        'will_sleep_ms' => $ms,
        'note'      => 'Response sent immediately; worker sleeps in background.',
    ]);
    oxphp_finish_request();
    usleep($ms * 1000);
    return;
}

usleep($ms * 1000);
json_response(200, [
    'slept_ms' => $ms,
    'note'     => 'Use REQUEST_TIMEOUT_SECONDS to test 504 Gateway Timeout.',
]);
