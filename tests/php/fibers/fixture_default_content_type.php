<?php

declare(strict_types=1);

// Inner request for fibers/test_default_content_type_stays_with_its_request.
// ?who= names it. ?type=json sets a Content-Type of its own. ?park=N then parks
// it for N ms before any output, publishing both edges of the park in
// $sharedState[?edge]. It answers whether the park ?watch names was in progress
// as it wrote its body.

if (($_GET['type'] ?? '') === 'json') {
    header('Content-Type: application/json');
}

$edge = (string) ($_GET['edge'] ?? '');
if (isset($_GET['park']) && $edge !== '') {
    $sharedState[$edge] = true;
    oxphp_usleep((int) $_GET['park'] * 1000);
    $sharedState[$edge] = false;
}

$watch = (string) ($_GET['watch'] ?? '');
echo json_encode([
    'who' => (string) ($_GET['who'] ?? '-'),
    'watched_parked' => $watch === '' ? null : ($sharedState[$watch] ?? null),
]);
