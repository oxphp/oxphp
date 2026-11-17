<?php

declare(strict_types=1);

// Inner request for fibers/test_implicit_flush_stays_with_its_request. ?sse=1
// makes it an event stream, whose output reaches its client only as it is
// flushed. ?set=on|off calls ob_implicit_flush() with that, and ?busy=N then
// keeps it busy for N ms without parking. ?prepark=N parks it for N ms before
// it writes anything, publishing both edges of the park in
// $sharedState[?preedge]. It then writes a frame, parks for ?park ms with its
// edges in $sharedState[?edge], and writes a last frame naming whether the park
// ?watch names was in progress then, when that park ended, and what
// thread_cell_queue_premise() read as it started. None of it calls flush().

require_once __DIR__ . '/thread_cell_probe.php';

$premise = thread_cell_queue_premise($sharedState);
$parkEndedAt = null;
$park = static function (string $ms, string $edge) use (&$sharedState, &$parkEndedAt): void {
    if (!isset($_GET[$ms], $_GET[$edge])) {
        return;
    }
    $sharedState[$_GET[$edge]] = true;
    oxphp_usleep((int) $_GET[$ms] * 1000);
    $sharedState[$_GET[$edge]] = false;
    $parkEndedAt = microtime(true);
};

if (isset($_GET['sse'])) {
    header('Content-Type: text/event-stream');
}
if (isset($_GET['set'])) {
    ob_implicit_flush($_GET['set'] === 'on');
}
if (isset($_GET['busy'])) {
    thread_cell_busy($sharedState, (int) $_GET['busy']);
}

$park('prepark', 'preedge');
echo "data: one\n\n";
$park('park', 'edge');

$watch = (string) ($_GET['watch'] ?? '');
echo 'data: ' . json_encode([
    'watched_parked' => $watch === '' ? null : ($sharedState[$watch] ?? null),
    'park_ended_at' => $parkEndedAt,
    'premise' => $premise,
]) . "\n\n";
