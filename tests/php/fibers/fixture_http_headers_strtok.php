<?php

declare(strict_types=1);

// Inner request for fibers/test_last_http_headers_and_strtok_stay_with_their_request.
// It first records what http_get_last_response_headers() and a strtok() that
// carries on return as it starts, with what thread_cell_queue_premise() reads.
// ?fetch= then fetches that path from the internal server, which answers
// without running PHP, and ?tok= starts a strtok() over its value. ?busy=N then
// keeps it busy for N ms without parking; ?park=N parks it for N ms instead,
// publishing both edges of the park in $sharedState[?edge]. It answers what it
// recorded, what the same two calls return at its end, and whether the park
// ?watch names was in progress then.

require_once __DIR__ . '/thread_cell_probe.php';

$cells = static function (): array {
    $type = null;
    foreach (http_get_last_response_headers() ?? [] as $line) {
        if (stripos($line, 'content-type:') === 0) {
            $type = strtolower(trim(substr($line, strlen('content-type:'))));
        }
    }

    return ['fetched_type' => $type, 'next_token' => thread_cell_strtok_next()];
};

$premise = thread_cell_queue_premise($sharedState);
$start = $cells();

if (isset($_GET['fetch'])) {
    $ctx = stream_context_create(['http' => ['timeout' => 3, 'ignore_errors' => true]]);
    @file_get_contents('http://127.0.0.1:9090/' . $_GET['fetch'], false, $ctx);
}
if (isset($_GET['tok'])) {
    strtok((string) $_GET['tok'], ' ');
}
if (isset($_GET['busy'])) {
    thread_cell_busy($sharedState, (int) $_GET['busy']);
}

$edge = (string) ($_GET['edge'] ?? '');
if (isset($_GET['park']) && $edge !== '') {
    $sharedState[$edge] = true;
    oxphp_usleep((int) $_GET['park'] * 1000);
    $sharedState[$edge] = false;
}

$watch = (string) ($_GET['watch'] ?? '');
echo json_encode([
    'premise' => $premise,
    'start' => $start,
    'end' => $cells(),
    'watched_parked' => $watch === '' ? null : ($sharedState[$watch] ?? null),
]);
