<?php

declare(strict_types=1);

// Inner request for fibers/test_output_start_stays_with_its_request and
// fibers/test_output_sent_stays_with_its_request. ?first=1 writes before the
// park, and every request writes after it, each write on a line of its own.
// ?busy=N keeps it busy for N ms without parking after its first write; ?park=N
// parks it for N ms there instead, publishing both edges of the park in
// $sharedState[?edge]. It answers, on its last line, the line it first wrote
// on, where headers_sent() says its output started, whether PHP still took a
// change to zlib.output_compression as it started and again after the park,
// whether the park ?watch names was in progress then, and what
// thread_cell_queue_premise() read as it started.

require_once __DIR__ . '/thread_cell_probe.php';

$premise = thread_cell_queue_premise($sharedState);
$changeableAtStart = thread_cell_compression_changeable();
$firstLine = null;
if (isset($_GET['first'])) {
    $firstLine = __LINE__; echo "first\n";
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
$changeableAfterPark = thread_cell_compression_changeable();

$lastLine = __LINE__; echo "last\n";

headers_sent($file, $line);
$watch = (string) ($_GET['watch'] ?? '');
echo json_encode([
    'wrote_at' => $firstLine ?? $lastLine,
    'started_at' => $line,
    'started_here' => $file === __FILE__,
    'changeable_at_start' => $changeableAtStart,
    'changeable_after_park' => $changeableAfterPark,
    'zlib' => extension_loaded('zlib'),
    'watched_parked' => $watch === '' ? null : ($sharedState[$watch] ?? null),
    'premise' => $premise,
]);
