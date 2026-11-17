<?php

declare(strict_types=1);

// Inner request for fibers/test_unserialize_table_stays_with_its_request.
//
// ?role=holder parks inside unserialize() with an object of its own in the
// call's table. The string is a whole array holding that object, followed by a
// stray byte: the parse succeeds, and the warning about the extra data is
// raised before the call frees its table, so the error handler that parks on it
// does so with the table live. Both edges of the park are published in
// $sharedState.
//
// ?role=neighbour unserializes a pair that refers to its own object twice and
// reports what came back, and whether the holder was parked meanwhile.
//
// The holder probes once more after its call has returned: the call has to end
// on the count it began with, or the next one on the thread finds a table that
// is not there.

require_once __DIR__ . '/thread_cell_probe.php';

ini_set('display_errors', '0');

if (($_GET['role'] ?? '') === 'neighbour') {
    $o = new stdClass();
    $o->who = 'neighbour';
    $pair = unserialize(serialize([$o, $o]));
    echo json_encode([
        'holder_parked' => $sharedState['cell_unser_parked'] ?? null,
        'same' => is_array($pair) && isset($pair[0], $pair[1]) && $pair[0] === $pair[1],
        'second_who' => is_array($pair) && isset($pair[1]) && is_object($pair[1]) ? ($pair[1]->who ?? null) : null,
    ]);

    return;
}

$own = new stdClass();
$own->who = 'holder';
set_error_handler(static function () use (&$sharedState): bool {
    $sharedState['cell_unser_parked'] = true;
    oxphp_usleep(600_000);
    $sharedState['cell_unser_parked'] = false;

    return true;
});
$result = unserialize(serialize([$own]) . 'X');
restore_error_handler();
$after = thread_cell_unserialize_probe();

echo json_encode([
    'result_who' => is_array($result) && isset($result[0]) && is_object($result[0]) ? ($result[0]->who ?? null) : null,
    'after' => $after,
]);
